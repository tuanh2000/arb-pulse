use alloy::sol;

// Mirrors contracts/src/ArbExecutor.sol. `sol!` generates the calldata encoder
// `executeArbitrageCall` (implements SolCall) and the `Hop` struct.
sol! {
    struct Hop {
        address pair;
        address tokenIn;
        uint16 feeBps;
    }

    function executeArbitrage(
        address token,
        uint256 amount,
        Hop[] hops,
        uint256 minProfit
    ) external;
}
