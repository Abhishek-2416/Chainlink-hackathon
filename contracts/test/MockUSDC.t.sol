// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../src/MockUSDC.sol";

contract MockUSDCTest is Test {
    MockUSDC usdc;
    address alice = makeAddr("alice");

    function setUp() public {
        usdc = new MockUSDC();
    }

    function test_decimals() public view {
        assertEq(usdc.decimals(), 6);
    }

    function test_faucet() public {
        vm.prank(alice);
        usdc.faucet(1_000_000); // 1 USDC
        assertEq(usdc.balanceOf(alice), 1_000_000);
    }

    function test_nameAndSymbol() public view {
        assertEq(usdc.name(), "Mock USDC");
        assertEq(usdc.symbol(), "mUSDC");
    }
}