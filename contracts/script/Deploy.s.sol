// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {ArbExecutor} from "../src/ArbExecutor.sol";

/// Minimal cheatcode interface (avoids a forge-std dependency).
interface Vm {
    function envOr(string calldata name, address defaultValue) external view returns (address);
    function startBroadcast() external;
    function stopBroadcast() external;
}

/// Deploy ArbExecutor against the PHUX (Balancer V2) Vault on PulseChain.
///   forge script script/Deploy.s.sol --rpc-url pulsechain --broadcast
/// Override the vault with the PHUX_VAULT env var if needed.
contract Deploy {
    Vm internal constant vm = Vm(0x7109709ECfa91a80626fF3989D68f67F5b1DD12D);

    /// PHUX Vault on PulseChain. Verified on-chain to expose the Balancer V2
    /// Vault interface (getAuthorizer / getProtocolFeesCollector / flashLoan /
    /// getPoolTokens). 2026-05-25.
    address internal constant PHUX_VAULT = 0x7F51AC3df6A034273FB09BB29e383FCF655e473c;

    function run() external returns (ArbExecutor executor) {
        address vault = vm.envOr("PHUX_VAULT", PHUX_VAULT);
        vm.startBroadcast();
        executor = new ArbExecutor(vault);
        vm.stopBroadcast();
    }
}
