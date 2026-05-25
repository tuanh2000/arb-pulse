// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {
    IERC20,
    IFlashLoanRecipient,
    IBalancerVault,
    IUniswapV2Pair
} from "./interfaces/IArbInterfaces.sol";

/// @title ArbExecutor
/// @notice Atomic cross-DEX arbitrage executor for PulseChain. Borrows the base
///         token via a PHUX (Balancer V2) flash loan, runs a Uniswap V2-style
///         swap path that returns to the base token, repays the loan, and keeps
///         the profit. Reverts (no loss) if the path is not profitable.
///
/// The path is produced off-chain by the opportunity-finder and submitted by the
/// Sender. The contract recomputes each hop's output from *live* reserves so the
/// inner `swap` calls don't revert on stale numbers, and enforces a single global
/// profit guard before repaying.
contract ArbExecutor is IFlashLoanRecipient {
    /// One swap in the cycle. `feeBps` must be >= the pool's real fee, otherwise
    /// the computed output is too high and the pair's k-invariant check reverts.
    struct Hop {
        address pair; // Uniswap V2-style pair
        address tokenIn; // token sent into this hop
        uint16 feeBps; // pool fee in basis points (e.g. 30 = 0.30%)
    }

    struct ArbParams {
        Hop[] hops;
        uint256 minProfit; // minimum profit in the borrowed token, raw units
    }

    IBalancerVault public immutable vault;
    address public owner;

    /// Set only for the duration of our own flash loan, so the callback can't be
    /// driven by a loan someone else initiated with this contract as recipient.
    bool private _loanActive;

    event ArbExecuted(address indexed token, uint256 amountBorrowed, uint256 profit);
    event OwnerChanged(address indexed previousOwner, address indexed newOwner);

    error NotOwner();
    error NotVault();
    error LoanNotInitiated();
    error EmptyPath();
    error Unprofitable(uint256 balanceAfter, uint256 required);
    error TransferFailed();
    error ZeroAddress();

    modifier onlyOwner() {
        if (msg.sender != owner) revert NotOwner();
        _;
    }

    constructor(address _vault) {
        if (_vault == address(0)) revert ZeroAddress();
        vault = IBalancerVault(_vault);
        owner = msg.sender;
        emit OwnerChanged(address(0), msg.sender);
    }

    function setOwner(address newOwner) external onlyOwner {
        if (newOwner == address(0)) revert ZeroAddress();
        emit OwnerChanged(owner, newOwner);
        owner = newOwner;
    }

    /// @notice Borrow `amount` of `token` from the vault and run `hops` as a cycle.
    /// @param token   Base token to borrow (== hops[0].tokenIn and the cycle's end token).
    /// @param amount  Amount to borrow (the finder's optimal size, clamped to liquidity).
    /// @param hops    Ordered swaps; the last hop must output `token`.
    /// @param minProfit Minimum profit (raw `token` units) required, else revert.
    function executeArbitrage(
        address token,
        uint256 amount,
        Hop[] calldata hops,
        uint256 minProfit
    ) external onlyOwner {
        if (hops.length == 0) revert EmptyPath();

        IERC20[] memory tokens = new IERC20[](1);
        tokens[0] = IERC20(token);
        uint256[] memory amounts = new uint256[](1);
        amounts[0] = amount;

        bytes memory userData = abi.encode(ArbParams({hops: hops, minProfit: minProfit}));

        _loanActive = true;
        vault.flashLoan(IFlashLoanRecipient(address(this)), tokens, amounts, userData);
        _loanActive = false;
    }

    /// @inheritdoc IFlashLoanRecipient
    function receiveFlashLoan(
        IERC20[] memory tokens,
        uint256[] memory amounts,
        uint256[] memory feeAmounts,
        bytes memory userData
    ) external override {
        if (msg.sender != address(vault)) revert NotVault();
        if (!_loanActive) revert LoanNotInitiated();

        ArbParams memory p = abi.decode(userData, (ArbParams));
        IERC20 token = tokens[0];
        uint256 amount = amounts[0];
        uint256 fee = feeAmounts[0];

        // Includes the just-received `amount`, plus any pre-existing balance.
        uint256 balanceBefore = token.balanceOf(address(this));

        uint256 amountIn = amount;
        for (uint256 i = 0; i < p.hops.length; i++) {
            amountIn = _swap(p.hops[i], amountIn);
        }

        uint256 balanceAfter = token.balanceOf(address(this));

        // proceeds = balanceAfter - (balanceBefore - amount); we need
        // proceeds >= amount + fee + minProfit, i.e. the guard below. This never
        // lets the arb be subsidized by the contract's pre-existing balance.
        uint256 required = balanceBefore + fee + p.minProfit;
        if (balanceAfter < required) revert Unprofitable(balanceAfter, required);

        // Repay principal + flash-loan fee (fee is 0 on PHUX today).
        _safeTransfer(address(token), address(vault), amount + fee);

        uint256 profit = balanceAfter - balanceBefore - fee;
        emit ArbExecuted(address(token), amount, profit);
    }

    /// Execute one V2 hop: compute output from live reserves, send input to the
    /// pair, and pull output to this contract.
    function _swap(Hop memory hop, uint256 amountIn) internal returns (uint256 amountOut) {
        IUniswapV2Pair pair = IUniswapV2Pair(hop.pair);
        (uint112 reserve0, uint112 reserve1,) = pair.getReserves();
        bool inIsToken0 = hop.tokenIn == pair.token0();

        (uint256 reserveIn, uint256 reserveOut) =
            inIsToken0 ? (uint256(reserve0), uint256(reserve1)) : (uint256(reserve1), uint256(reserve0));

        uint256 amountInWithFee = amountIn * (10_000 - hop.feeBps);
        amountOut = (amountInWithFee * reserveOut) / (reserveIn * 10_000 + amountInWithFee);

        _safeTransfer(hop.tokenIn, hop.pair, amountIn);

        (uint256 amount0Out, uint256 amount1Out) =
            inIsToken0 ? (uint256(0), amountOut) : (amountOut, uint256(0));
        pair.swap(amount0Out, amount1Out, address(this), new bytes(0));
    }

    // --- Owner fund management ---------------------------------------------

    function withdraw(address token, uint256 amount) external onlyOwner {
        _safeTransfer(token, owner, amount);
    }

    function withdrawAll(address token) external onlyOwner {
        _safeTransfer(token, owner, IERC20(token).balanceOf(address(this)));
    }

    function withdrawNative() external onlyOwner {
        (bool ok,) = owner.call{value: address(this).balance}("");
        if (!ok) revert TransferFailed();
    }

    receive() external payable {}

    // --- Internal helpers ---------------------------------------------------

    function _safeTransfer(address token, address to, uint256 value) internal {
        (bool ok, bytes memory data) =
            token.call(abi.encodeWithSelector(IERC20.transfer.selector, to, value));
        if (!ok || (data.length != 0 && !abi.decode(data, (bool)))) revert TransferFailed();
    }
}
