// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {IERC20, IUniswapV2Pair} from "./interfaces/IArbInterfaces.sol";

/// @title FotDetector
/// @notice Fee-on-transfer detector run via `eth_call` with state overrides:
///         the caller deploys this runtime code at a scratch address and funds
///         that address with a large `base`-token balance (by overriding the
///         base token's balance slot). `measure` reads the pair's reserves,
///         deposits the full base reserve, and performs a normal Uniswap-V2 swap
///         to pull a slice of `tokenOut` to itself, then returns how much
///         actually arrived. A standard ERC20 delivers exactly the requested
///         amount; a fee-on-transfer / tax token delivers less. No real
///         deployment, no funds at risk.
contract FotDetector {
    /// @param pair     V2 pair holding (base, tokenOut)
    /// @param base     funding token (the pair's other side); caller funds this contract with it
    /// @param tokenOut token under test
    /// @return requested amount of tokenOut requested out of the pair
    /// @return received  amount of tokenOut actually delivered to this contract
    function measure(address pair, address base, address tokenOut)
        external
        returns (uint256 requested, uint256 received)
    {
        (uint112 r0, uint112 r1,) = IUniswapV2Pair(pair).getReserves();
        bool outIsToken0 = IUniswapV2Pair(pair).token0() == tokenOut;
        uint256 tokenOutReserve = outIsToken0 ? r0 : r1;
        uint256 baseReserve = outIsToken0 ? r1 : r0;

        uint256 amountOut = tokenOutReserve / 1000;
        if (amountOut == 0) amountOut = 1;

        // Depositing the whole base reserve keeps the k-invariant satisfied for
        // the small (0.1% of reserve) output we request.
        IERC20(base).transfer(pair, baseReserve);
        (uint256 a0, uint256 a1) =
            outIsToken0 ? (amountOut, uint256(0)) : (uint256(0), amountOut);
        IUniswapV2Pair(pair).swap(a0, a1, address(this), "");

        requested = amountOut;
        received = IERC20(tokenOut).balanceOf(address(this));
    }
}
