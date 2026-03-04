// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "forge-std/Script.sol";
import "../src/PredictionMarket.sol";

contract CreateMarketScript is Script {
    function run() external {
        uint256 deployerPrivateKey = vm.envUint("PRIVATE_KEY");

        // Your deployed PredictionMarket address
        PredictionMarket market = PredictionMarket(0x45e7911Af8c31bDeDf8A586BeEd8efEcACEb9c37);

        vm.startBroadcast(deployerPrivateKey);

        uint256 marketId = market.createMarket(
            keccak256("Will BTC hit 100k by Dec 2025?"),
            block.timestamp + 30 days
        );

        console.log("Market created with ID:", marketId);

        vm.stopBroadcast();
    }
}