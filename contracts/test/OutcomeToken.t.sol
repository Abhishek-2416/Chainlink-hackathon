// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../src/OutcomeToken.sol";

contract OutcomeTokenTest is Test {
    OutcomeToken token;
    address alice = makeAddr("alice");
    address bob = makeAddr("bob");

    function setUp() public {
        // this test contract acts as the "factory"
        token = new OutcomeToken("BTC 100k? - YES", "YES-1");
    }

    function test_factoryIsDeployer() public view {
        assertEq(token.factory(), address(this));
    }

    function test_mintByFactory() public {
        token.mint(alice, 1_000_000);
        assertEq(token.balanceOf(alice), 1_000_000);
    }

    function test_mintByNonFactoryReverts() public {
        vm.prank(alice);
        vm.expectRevert("OutcomeToken: caller is not factory");
        token.mint(alice, 1_000_000);
    }

    function test_burnByFactory() public {
        token.mint(alice, 1_000_000);
        token.burn(alice, 1_000_000);
        assertEq(token.balanceOf(alice), 0);
    }

    function test_usersCanTransfer() public {
        token.mint(alice, 1_000_000);
        vm.prank(alice);
        token.transfer(bob, 500_000);
        assertEq(token.balanceOf(alice), 500_000);
        assertEq(token.balanceOf(bob), 500_000);
    }
}