// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "@openzeppelin/contracts/token/ERC20/ERC20.sol";

contract MockUSDC is ERC20 {
    uint8 private constant DECIMALS = 6;

    constructor() ERC20("Mock USDC", "mUSDC") {}

    function decimals() public pure override returns (uint8) {
        return DECIMALS;
    }

    /// @notice Anyone can mint test tokens to themselves
    /// @param amount Amount in smallest unit (6 decimals, so 1 USDC = 1_000_000)
    function faucet(uint256 amount) external {
        _mint(msg.sender, amount);
    }
}