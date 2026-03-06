// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import "./OutcomeToken.sol";
import "./IReceiver.sol";

/// @title PredictionMarket
/// @notice Decentralized prediction market with CRE-powered AI resolution
/// @dev Implements IReceiver so Chainlink Forwarder can deliver resolution reports
contract PredictionMarket is IReceiver {
    // ── Types ──────────────────────────────────────────
    enum MarketStatus { Open, Resolved, Cancelled }
    enum Outcome { Unresolved, Yes, No }

    struct Market {
        bytes32 questionHash;
        address creator;
        OutcomeToken yesToken;
        OutcomeToken noToken;
        uint256 resolutionTimestamp;
        uint256 totalCollateral;
        MarketStatus status;
        Outcome outcome;
    }

    // ── State ──────────────────────────────────────────
    IERC20 public immutable collateralToken;  // MockUSDC
    address public forwarder;                  // Chainlink Forwarder address
    address public owner;

    uint256 public marketCount;
    mapping(uint256 => Market) public markets;
    mapping(bytes32 => bool) public questionExists;

    // ── Events ─────────────────────────────────────────
    event MarketCreated(
        uint256 indexed marketId,
        bytes32 questionHash,
        address creator,
        address yesToken,
        address noToken,
        uint256 resolutionTimestamp
    );
    event TokensMinted(uint256 indexed marketId, address indexed user, uint256 amount);
    event MarketResolved(uint256 indexed marketId, Outcome outcome);
    event WinningsRedeemed(uint256 indexed marketId, address indexed user, uint256 amount);

    // ── Modifiers ──────────────────────────────────────
    modifier onlyForwarder() {
        require(msg.sender == forwarder, "Only Chainlink Forwarder");
        _;
    }

    modifier onlyOwner() {
        require(msg.sender == owner, "Not owner");
        _;
    }

    // ── Constructor ────────────────────────────────────
    /// @param _collateralToken Address of MockUSDC (or real USDC)
    /// @param _forwarder Address of Chainlink Forwarder on this network
    ///        For simulation: MockKeystoneForwarder address
    ///        For production: KeystoneForwarder address
    ///        See: https://docs.chain.link/cre/guides/workflow/using-evm-client/forwarder-directory
    constructor(address _collateralToken, address _forwarder) {
        collateralToken = IERC20(_collateralToken);
        forwarder = _forwarder;
        owner = msg.sender;
    }

    // ════════════════════════════════════════════════════
    // MARKET CREATION
    // ════════════════════════════════════════════════════

    /// @notice Create a new prediction market
    /// @param questionHash keccak256 of the question text (stored off-chain)
    /// @param resolutionTimestamp When the market can be resolved
    function createMarket(
        bytes32 questionHash,
        uint256 resolutionTimestamp
    ) external returns (uint256) {
        require(resolutionTimestamp > block.timestamp, "Resolution must be in future");
        require(!questionExists[questionHash], "Market already exists");

        uint256 marketId = marketCount++;

        // Deploy YES and NO token contracts
        OutcomeToken yesToken = new OutcomeToken(
            string.concat("YES-", _uint2str(marketId)),
            string.concat("YES-", _uint2str(marketId))
        );
        OutcomeToken noToken = new OutcomeToken(
            string.concat("NO-", _uint2str(marketId)),
            string.concat("NO-", _uint2str(marketId))
        );

        markets[marketId] = Market({
            questionHash: questionHash,
            creator: msg.sender,
            yesToken: yesToken,
            noToken: noToken,
            resolutionTimestamp: resolutionTimestamp,
            totalCollateral: 0,
            status: MarketStatus.Open,
            outcome: Outcome.Unresolved
        });

        questionExists[questionHash] = true;

        emit MarketCreated(
            marketId, questionHash, msg.sender,
            address(yesToken), address(noToken),
            resolutionTimestamp
        );

        return marketId;
    }

    // ════════════════════════════════════════════════════
    // TOKEN MINTING (called by backend during order matching)
    // ════════════════════════════════════════════════════

    /// @notice Mint a YES + NO token pair by depositing collateral
    /// @dev Backend calls this when matching orders. User pays USDC,
    ///      gets both tokens. Backend then transfers the right token
    ///      to each side of the trade.
    /// @param marketId Which market to mint for
    /// @param to Who receives the minted tokens
    /// @param amount How many token pairs to mint (1:1 with collateral)
    function mintTokens(uint256 marketId, address to, uint256 amount) external {
        Market storage market = markets[marketId];
        require(market.status == MarketStatus.Open, "Market not open");

        // Take collateral (USDC) from caller
        bool success = collateralToken.transferFrom(msg.sender, address(this), amount);
        require(success, "USDC transfer failed");

        // Mint both outcome tokens to recipient
        market.yesToken.mint(to, amount);
        market.noToken.mint(to, amount);
        market.totalCollateral += amount;

        emit TokensMinted(marketId, to, amount);
    }

    // ════════════════════════════════════════════════════
    // CRE RESOLUTION (Chainlink Forwarder → onReport)
    // ════════════════════════════════════════════════════

    /// @notice Called by Chainlink Forwarder when CRE workflow submits a report
    /// @dev This is the IReceiver interface implementation.
    ///      Flow: CRE workflow → runtime.report() → evmClient.writeReport()
    ///            → Forwarder verifies DON signatures
    ///            → Forwarder calls this onReport()
    /// @param metadata Workflow metadata (workflowId, owner, etc.)
    /// @param report ABI-encoded report data: (uint256 marketId, uint8 outcome)
    function onReport(bytes calldata metadata, bytes calldata report) external onlyForwarder {
        // Decode the resolution data from the CRE workflow
        (uint256 marketId, uint8 outcomeRaw) = abi.decode(report, (uint256, uint8));

        Outcome outcome = Outcome(outcomeRaw);

        Market storage market = markets[marketId];
        require(market.status == MarketStatus.Open, "Market not open");
        require(outcome == Outcome.Yes || outcome == Outcome.No, "Invalid outcome");

        market.status = MarketStatus.Resolved;
        market.outcome = outcome;

        emit MarketResolved(marketId, outcome);
    }

    // ════════════════════════════════════════════════════
    // REDEMPTION
    // ════════════════════════════════════════════════════

    /// @notice Redeem winning tokens for collateral (USDC)
    /// @dev 1 winning token = 1 USDC. Losing tokens are worthless.
    /// @param marketId Which market to redeem from
    /// @param amount How many winning tokens to redeem
    function redeemWinning(uint256 marketId, uint256 amount) external {
        Market storage market = markets[marketId];
        require(market.status == MarketStatus.Resolved, "Market not resolved");

        // Determine which token won
        OutcomeToken winningToken;
        if (market.outcome == Outcome.Yes) {
            winningToken = market.yesToken;
        } else {
            winningToken = market.noToken;
        }

        require(winningToken.balanceOf(msg.sender) >= amount, "Insufficient tokens");

        // Burn winning tokens
        winningToken.burn(msg.sender, amount);

        // Pay out collateral
        market.totalCollateral -= amount;
        bool success = collateralToken.transfer(msg.sender, amount);
        require(success, "USDC transfer failed");

        emit WinningsRedeemed(marketId, msg.sender, amount);
    }

    // ════════════════════════════════════════════════════
    // ADMIN
    // ════════════════════════════════════════════════════

    /// @notice Update forwarder address (e.g., switching from simulation to production)
    function setForwarder(address _forwarder) external onlyOwner {
        forwarder = _forwarder;
    }

    // ════════════════════════════════════════════════════
    // VIEW HELPERS
    // ════════════════════════════════════════════════════

    function getMarket(uint256 marketId) external view returns (Market memory) {
        return markets[marketId];
    }

    function getTokenAddresses(uint256 marketId) external view returns (address yes, address no) {
        Market storage market = markets[marketId];
        return (address(market.yesToken), address(market.noToken));
    }

    // ── Internal ───────────────────────────────────────
    function _uint2str(uint256 value) internal pure returns (string memory) {
        if (value == 0) return "0";
        uint256 temp = value;
        uint256 digits;
        while (temp != 0) {
            digits++;
            temp /= 10;
        }
        bytes memory buffer = new bytes(digits);
        while (value != 0) {
            digits--;
            buffer[digits] = bytes1(uint8(48 + (value % 10)));
            value /= 10;
        }
        return string(buffer);
    }
}
