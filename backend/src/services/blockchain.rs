use anyhow::Result;
use ethers::{
    abi::{encode, Token},
    prelude::*,
    types::{Address, Bytes, H256, TransactionRequest, U256},
    utils::keccak256,
};
use std::sync::Arc;

// ── EIP-712 constants (must match PredictionMarket.sol exactly) ──────────────

const EIP712_DOMAIN_TYPEHASH: &str =
    "EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)";

const ORDER_TYPEHASH: &str =
    "Order(uint256 marketId,uint8 outcome,address to,uint256 shares,uint256 cost,uint256 deadline,uint256 nonce)";

/// ABI-encoded function selector for `nonces(address)`.
fn nonces_selector() -> [u8; 4] {
    keccak256("nonces(address)")[..4].try_into().unwrap()
}

/// ABI-encoded function selector for
/// `fillOrder((uint256,uint8,address,uint256,uint256,uint256,uint256),bytes)`.
fn fill_order_selector() -> [u8; 4] {
    keccak256(
        "fillOrder((uint256,uint8,address,uint256,uint256,uint256,uint256),bytes)",
    )[..4]
    .try_into()
    .unwrap()
}

// ── Order struct ──────────────────────────────────────────────────────────────

/// Named order struct — matches the Solidity `Order` struct field-for-field.
#[derive(Clone, Debug)]
pub struct ContractOrder {
    pub market_id: U256,
    pub outcome: u8,   // 1 = Yes, 2 = No  (Outcome enum in Solidity)
    pub to: Address,
    pub shares: U256,
    pub cost: U256,
    pub deadline: U256,
    pub nonce: U256,
}

impl ContractOrder {
    /// EIP-712 struct hash — equivalent to `contract.orderDigest(order)`.
    pub fn struct_hash(&self) -> [u8; 32] {
        let typehash = keccak256(ORDER_TYPEHASH.as_bytes());

        // ABI-encode exactly as the contract does:
        //   abi.encode(ORDER_TYPEHASH, marketId, outcome, to, shares, cost, deadline, nonce)
        let encoded = encode(&[
            Token::FixedBytes(typehash.to_vec()),
            Token::Uint(self.market_id),
            Token::Uint(U256::from(self.outcome)),
            Token::Address(self.to),
            Token::Uint(self.shares),
            Token::Uint(self.cost),
            Token::Uint(self.deadline),
            Token::Uint(self.nonce),
        ]);
        keccak256(encoded)
    }

    /// Full EIP-712 digest — what the user signs and what the contract checks.
    pub fn eip712_digest(&self, domain_separator: &[u8; 32]) -> [u8; 32] {
        let mut payload = Vec::with_capacity(66);
        payload.extend_from_slice(b"\x19\x01");
        payload.extend_from_slice(domain_separator);
        payload.extend_from_slice(&self.struct_hash());
        keccak256(payload)
    }

    /// ABI-encode the order as a Solidity tuple — used in fillOrder calldata.
    fn abi_encode_tuple(&self) -> Vec<Token> {
        vec![
            Token::Uint(self.market_id),
            Token::Uint(U256::from(self.outcome)),
            Token::Address(self.to),
            Token::Uint(self.shares),
            Token::Uint(self.cost),
            Token::Uint(self.deadline),
            Token::Uint(self.nonce),
        ]
    }
}

// ── BlockchainService ─────────────────────────────────────────────────────────

pub struct BlockchainService {
    provider: Arc<Provider<Http>>,
    wallet: Option<LocalWallet>,
    contract_address: Address,
    domain_separator: [u8; 32],
    chain_id: u64,
}

impl BlockchainService {
    pub async fn new() -> Result<Self> {
        let rpc_url = std::env::var("RPC_URL")
            .unwrap_or_else(|_| "https://ethereum-sepolia-rpc.publicnode.com".into());
        let contract_addr = std::env::var("PREDICTION_MARKET_ADDRESS")
            .unwrap_or_else(|_| "0x45e7911Af8c31bDeDf8A586BeEd8efEcACEb9c37".into());

        let provider = Arc::new(Provider::<Http>::try_from(rpc_url)?);
        let chain_id = provider.get_chainid().await?.as_u64();
        let contract_address: Address = contract_addr.parse()?;

        let wallet = match std::env::var("BACKEND_PRIVATE_KEY") {
            Ok(pk) => {
                let w = pk.parse::<LocalWallet>()?.with_chain_id(chain_id);
                tracing::info!("Backend wallet loaded: {:?}", w.address());
                Some(w)
            }
            Err(_) => {
                tracing::warn!("BACKEND_PRIVATE_KEY not set — fillOrder will be queued");
                None
            }
        };

        // Compute the EIP-712 domain separator once at startup.
        let domain_separator = Self::compute_domain_separator(chain_id, contract_address);

        Ok(Self {
            provider,
            wallet,
            contract_address,
            domain_separator,
            chain_id,
        })
    }

    /// EIP-712 domain separator — matches `EIP712._domainSeparatorV4()` in the contract.
    fn compute_domain_separator(chain_id: u64, contract_address: Address) -> [u8; 32] {
        let typehash = keccak256(EIP712_DOMAIN_TYPEHASH.as_bytes());
        let name_hash = keccak256("PredictionMarket".as_bytes());
        let version_hash = keccak256("1".as_bytes());

        let encoded = encode(&[
            Token::FixedBytes(typehash.to_vec()),
            Token::FixedBytes(name_hash.to_vec()),
            Token::FixedBytes(version_hash.to_vec()),
            Token::Uint(U256::from(chain_id)),
            Token::Address(contract_address),
        ]);
        keccak256(encoded)
    }

    pub fn chain_id(&self) -> u64 {
        self.chain_id
    }

    /// On-chain nonce for `user_address` — call `nonces(address)` via eth_call.
    pub async fn get_nonce(&self, user_address: &str) -> Result<u64> {
        let user: Address = user_address.parse()?;

        // calldata = selector(4) + abi.encode(address)(32)
        let mut calldata = nonces_selector().to_vec();
        calldata.extend_from_slice(&encode(&[Token::Address(user)]));

        let result = self
            .provider
            .call(
                &TransactionRequest::new()
                    .to(self.contract_address)
                    .data(calldata)
                    .into(),
                None,
            )
            .await?;

        let n = U256::from_big_endian(&result);
        Ok(n.as_u64())
    }

    /// EIP-712 order digest — computed locally using the pre-computed domain separator.
    /// This is identical to calling `contract.orderDigest(order)` but requires no RPC call.
    pub fn get_order_digest(&self, order: &ContractOrder) -> [u8; 32] {
        order.eip712_digest(&self.domain_separator)
    }

    /// Submit fillOrder on-chain as the backend owner wallet.
    pub async fn fill_order(&self, order: ContractOrder, signature: Bytes) -> Result<H256> {
        let wallet = self
            .wallet
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("BACKEND_PRIVATE_KEY not set"))?;

        // calldata = selector(4) + abi.encode(tuple(order), signature)
        let mut calldata = fill_order_selector().to_vec();
        calldata.extend_from_slice(&encode(&[
            Token::Tuple(order.abi_encode_tuple()),
            Token::Bytes(signature.to_vec()),
        ]));

        let nonce = self.provider.get_transaction_count(wallet.address(), None).await?;
        let gas_price = self.provider.get_gas_price().await?;

        let tx = TransactionRequest::new()
            .to(self.contract_address)
            .data(calldata)
            .nonce(nonce)
            .gas_price(gas_price)
            .gas(500_000u64)
            .chain_id(self.chain_id);

        let signed = wallet.sign_transaction(&tx.clone().into()).await?;
        let raw = tx
            .rlp_signed(&signed);
        let pending = self.provider.send_raw_transaction(raw).await?;
        let receipt = pending
            .await?
            .ok_or_else(|| anyhow::anyhow!("Transaction dropped from mempool"))?;

        Ok(receipt.transaction_hash)
    }

    pub fn build_order(
        &self,
        market_id: u64,
        outcome: u8,
        to: &str,
        shares: u64,
        cost: u64,
        deadline: u64,
        nonce: u64,
    ) -> ContractOrder {
        ContractOrder {
            market_id: U256::from(market_id),
            outcome,
            to: to.parse().expect("invalid address"),
            shares: U256::from(shares),
            cost: U256::from(cost),
            deadline: U256::from(deadline),
            nonce: U256::from(nonce),
        }
    }
}
