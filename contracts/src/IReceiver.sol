// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @title IReceiver - Chainlink CRE Receiver Interface
/// @notice Your consumer contract must implement this interface
/// @dev The Chainlink Forwarder calls onReport() to deliver workflow data
interface IReceiver {
    function onReport(bytes calldata metadata, bytes calldata report) external;
}
