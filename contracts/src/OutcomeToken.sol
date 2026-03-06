// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "@openzeppelin/contracts/token/ERC20/ERC20.sol";

contract OutcomeToken is ERC20 {
    uint8 private constant DECIMALS = 6;
    address public immutable factory;

    modifier onlyFactory() {
        require(msg.sender == factory, "OutcomeToken: caller is not factory");
        _;
    }

    constructor(
        string memory name,
        string memory symbol
    ) ERC20(name, symbol) {
        factory = msg.sender; // whoever deploys this IS the factory
    }

    function decimals() public pure override returns (uint8) {
        return DECIMALS;
    }

    /// @notice Mint tokens — only callable by MarketFactory
    function mint(address to, uint256 amount) external onlyFactory {
        _mint(to, amount);
    }

    /// @notice Burn tokens — only callable by MarketFactory
    function burn(address from, uint256 amount) external onlyFactory {
        _burn(from, amount);
    }
}