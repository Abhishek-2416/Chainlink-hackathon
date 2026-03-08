// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "forge-std/Script.sol";
import "../src/MockUSDC.sol";
import "../src/PredictionMarket.sol";

contract DeployScript is Script {
    function run() external {
        // uint256 deployerPrivateKey = vm.envUint("PRIVATE_KEY");
        // address forwarder = vm.envAddress("FORWARDER_ADDRESS");
        address forwarder = 0x15fC6ae953E024d975e77382eEeC56A9101f9F88;

        vm.startBroadcast();

        // 1. Deploy MockUSDC
        MockUSDC usdc = new MockUSDC();
        console.log("MockUSDC deployed:", address(usdc));

        // 2. Deploy PredictionMarket
        // address owner = vm.addr();
        address owner = 0x834D5b0708ab331366f2a409e29F01222fc3AD6a;
        PredictionMarket market = new PredictionMarket(address(usdc), forwarder, owner);
        console.log("PredictionMarket deployed:", address(market));

        vm.stopBroadcast();
    }
}
