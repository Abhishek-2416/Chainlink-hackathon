// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../src/MockUSDC.sol";
import "../src/OutcomeToken.sol";
import "../src/PredictionMarket.sol";

contract PredictionMarketTest is Test {
    MockUSDC usdc;
    PredictionMarket market;

    address forwarder = makeAddr("chainlink-forwarder");
    address alice = makeAddr("alice");
    address bob = makeAddr("bob");
    address creator = makeAddr("creator");

    function setUp() public {
        usdc = new MockUSDC();
        market = new PredictionMarket(address(usdc), forwarder);
    }

    // ── Market Creation ────────────────────────────────
    function test_createMarket() public {
        vm.prank(creator);
        uint256 marketId = market.createMarket(
            keccak256("Will BTC hit 100k?"),
            block.timestamp + 30 days
        );
        assertEq(marketId, 0);

        PredictionMarket.Market memory m = market.getMarket(0);
        assertEq(m.creator, creator);
        assertEq(uint8(m.status), uint8(PredictionMarket.MarketStatus.Open));
        assertEq(uint8(m.outcome), uint8(PredictionMarket.Outcome.Unresolved));
    }

    function test_cannotCreateDuplicateMarket() public {
        bytes32 qHash = keccak256("Will BTC hit 100k?");
        market.createMarket(qHash, block.timestamp + 30 days);

        vm.expectRevert("Market already exists");
        market.createMarket(qHash, block.timestamp + 30 days);
    }

    function test_cannotCreatePastResolution() public {
        vm.expectRevert("Resolution must be in future");
        market.createMarket(keccak256("old question"), block.timestamp - 1);
    }

    // ── Minting ────────────────────────────────────────
    function test_mintTokens() public {
        market.createMarket(keccak256("BTC 100k?"), block.timestamp + 30 days);
        PredictionMarket.Market memory m = market.getMarket(0);

        // Alice gets USDC and approves
        vm.startPrank(alice);
        usdc.faucet(1_000_000); // 1 USDC
        usdc.approve(address(market), 1_000_000);
        market.mintTokens(0, alice, 1_000_000);
        vm.stopPrank();

        // Alice now has both tokens
        assertEq(OutcomeToken(address(m.yesToken)).balanceOf(alice), 1_000_000);
        assertEq(OutcomeToken(address(m.noToken)).balanceOf(alice), 1_000_000);
        // USDC locked in contract
        assertEq(usdc.balanceOf(address(market)), 1_000_000);
    }

    // ── CRE Resolution via onReport ────────────────────
    function test_resolveViaOnReport() public {
        market.createMarket(keccak256("BTC 100k?"), block.timestamp + 30 days);

        // Encode the same way CRE workflow would
        bytes memory report = abi.encode(
            uint256(0),  // marketId
            uint8(1)     // outcome = Yes
        );
        bytes memory metadata = ""; // simplified for testing

        // Only forwarder can call
        vm.prank(forwarder);
        market.onReport(metadata, report);

        PredictionMarket.Market memory m = market.getMarket(0);
        assertEq(uint8(m.status), uint8(PredictionMarket.MarketStatus.Resolved));
        assertEq(uint8(m.outcome), uint8(PredictionMarket.Outcome.Yes));
    }

    function test_nonForwarderCannotResolve() public {
        market.createMarket(keccak256("BTC 100k?"), block.timestamp + 30 days);

        bytes memory report = abi.encode(uint256(0), uint8(1));

        vm.prank(alice); // not the forwarder
        vm.expectRevert("Only Chainlink Forwarder");
        market.onReport("", report);
    }

    // ── Full Flow: Create → Mint → Trade → Resolve → Redeem ──
    function test_fullFlow() public {
        // 1. Creator makes a market
        vm.prank(creator);
        market.createMarket(keccak256("BTC 100k?"), block.timestamp + 30 days);
        PredictionMarket.Market memory m = market.getMarket(0);

        // 2. Alice mints tokens (she believes YES)
        vm.startPrank(alice);
        usdc.faucet(10_000_000); // 10 USDC
        usdc.approve(address(market), 10_000_000);
        market.mintTokens(0, alice, 10_000_000);
        vm.stopPrank();

        // 3. Alice sells her NO tokens to Bob (simulating orderbook match)
        vm.prank(alice);
        OutcomeToken(address(m.noToken)).transfer(bob, 10_000_000);

        // Alice: 10 YES, 0 NO
        // Bob:   0 YES, 10 NO
        assertEq(OutcomeToken(address(m.yesToken)).balanceOf(alice), 10_000_000);
        assertEq(OutcomeToken(address(m.noToken)).balanceOf(bob), 10_000_000);

        // 4. CRE workflow resolves: YES wins
        bytes memory report = abi.encode(uint256(0), uint8(1));
        vm.prank(forwarder);
        market.onReport("", report);

        // 5. Alice redeems — she wins
        vm.prank(alice);
        market.redeemWinning(0, 10_000_000);
        assertEq(usdc.balanceOf(alice), 10_000_000); // got her USDC back

        // 6. Bob holds NO tokens — worth nothing
        assertEq(OutcomeToken(address(m.noToken)).balanceOf(bob), 10_000_000);
        // Bob can't redeem NO tokens when YES won
    }
}