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

/// Extract the first 4 bytes of a keccak256 hash as a function selector.
fn selector(signature: &str) -> [u8; 4] {
    let hash = keccak256(signature.as_bytes());
    [hash[0], hash[1], hash[2], hash[3]]
}

/// ABI-encoded function selector for `nonces(address)`.
fn nonces_selector() -> [u8; 4] {
    selector("nonces(address)")
}

/// ABI-encoded function selector for `balanceOf(address)`.
fn balance_of_selector() -> [u8; 4] {
    keccak256("balanceOf(address)")[..4].try_into().unwrap()
}

/// ABI-encoded function selector for `createMarket(bytes32,uint256)`.
fn create_market_selector() -> [u8; 4] {
    selector("createMarket(bytes32,uint256)")
}

/// ABI-encoded function selector for `marketCount()`.
fn market_count_selector() -> [u8; 4] {
    keccak256("marketCount()")[..4].try_into().unwrap()
}

/// ABI-encoded function selector for
/// `fillOrder((uint256,uint8,address,uint256,uint256,uint256,uint256),bytes)`.
fn fill_order_selector() -> [u8; 4] {
    selector("fillOrder((uint256,uint8,address,uint256,uint256,uint256,uint256),bytes)")
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

    /// ETH balance (in wei) for an address.
    pub async fn get_eth_balance(&self, address: &str) -> Result<U256> {
        let addr: Address = address.parse()?;
        let balance = self.provider.get_balance(addr, None).await?;
        Ok(balance)
    }

    /// ERC-20 balance for a token contract. Returns raw units (e.g. 6 decimals for USDC).
    pub async fn get_erc20_balance(&self, token_address: &str, owner_address: &str) -> Result<U256> {
        let token: Address = token_address.parse()?;
        let owner: Address = owner_address.parse()?;

        let mut calldata = balance_of_selector().to_vec();
        calldata.extend_from_slice(&encode(&[Token::Address(owner)]));

        let result = self
            .provider
            .call(
                &TransactionRequest::new()
                    .to(token)
                    .data(calldata)
                    .into(),
                None,
            )
            .await?;

        Ok(U256::from_big_endian(&result))
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

    /// Read the on-chain `marketCount` — the ID that the next `createMarket` call will assign.
    pub async fn get_market_count(&self) -> Result<u64> {
        let calldata = market_count_selector().to_vec();

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

    /// Call `createMarket(bytes32 questionHash, uint256 resolutionTimestamp)` on-chain.
    /// Returns the transaction hash on success.
    pub async fn create_market(
        &self,
        question_hash: [u8; 32],
        resolution_timestamp: u64,
    ) -> Result<H256> {
        let wallet = self
            .wallet
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("BACKEND_PRIVATE_KEY not set"))?;

        let mut calldata = create_market_selector().to_vec();
        calldata.extend_from_slice(&encode(&[
            Token::FixedBytes(question_hash.to_vec()),
            Token::Uint(U256::from(resolution_timestamp)),
        ]));

        let nonce = self
            .provider
            .get_transaction_count(wallet.address(), None)
            .await?;
        let gas_price = U256::from(200_000_000u64);

        let tx = TransactionRequest::new()
            .to(self.contract_address)
            .data(calldata)
            .nonce(nonce)
            .gas_price(gas_price)
            .gas(2_000_000u64) // market creation deploys two ERC-20s
            .chain_id(self.chain_id);

        let signed = wallet.sign_transaction(&tx.clone().into()).await?;
        let raw = tx.rlp_signed(&signed);
        let pending = self.provider.send_raw_transaction(raw).await?;
        let receipt = pending
            .await?
            .ok_or_else(|| anyhow::anyhow!("Transaction dropped from mempool"))?;

        Ok(receipt.transaction_hash)
    }

    /// ABI-encoded function selector for
    /// `batchFillOrders((uint256,uint8,address,uint256,uint256,uint256,uint256)[],bytes[])`.
    fn batch_fill_orders_selector() -> [u8; 4] {
        selector("batchFillOrders((uint256,uint8,address,uint256,uint256,uint256,uint256)[],bytes[])")
    }

    /// ABI-encoded function selector for `cancelMarket(uint256)`.
    fn cancel_market_selector() -> [u8; 4] {
        selector("cancelMarket(uint256)")
    }

    /// Submit batchFillOrders on-chain as the backend owner wallet.
    pub async fn batch_fill_orders(
        &self,
        orders: Vec<ContractOrder>,
        signatures: Vec<Bytes>,
    ) -> Result<H256> {
        let wallet = self
            .wallet
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("BACKEND_PRIVATE_KEY not set"))?;

        if orders.len() != signatures.len() {
            return Err(anyhow::anyhow!("orders and signatures length mismatch"));
        }

        let order_tuples: Vec<Token> = orders
            .iter()
            .map(|o| Token::Tuple(o.abi_encode_tuple()))
            .collect();
        let sig_bytes: Vec<Token> = signatures
            .iter()
            .map(|s| Token::Bytes(s.to_vec()))
            .collect();

        let mut calldata = Self::batch_fill_orders_selector().to_vec();
        calldata.extend_from_slice(&encode(&[
            Token::Array(order_tuples),
            Token::Array(sig_bytes),
        ]));

        let nonce = self.provider.get_transaction_count(wallet.address(), None).await?;
        let gas_price = self.provider.get_gas_price().await?;

        let tx = TransactionRequest::new()
            .to(self.contract_address)
            .data(calldata)
            .nonce(nonce)
            .gas_price(gas_price)
            .gas(500_000u64 * orders.len() as u64) // scale gas with batch size
            .chain_id(self.chain_id);

        let signed = wallet.sign_transaction(&tx.clone().into()).await?;
        let raw = tx.rlp_signed(&signed);
        let pending = self.provider.send_raw_transaction(raw).await?;
        let receipt = pending
            .await?
            .ok_or_else(|| anyhow::anyhow!("Transaction dropped from mempool"))?;

        Ok(receipt.transaction_hash)
    }

    /// Call `cancelMarket(uint256 marketId)` on-chain. Owner only.
    pub async fn cancel_market(&self, market_id: u64) -> Result<H256> {
        let wallet = self
            .wallet
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("BACKEND_PRIVATE_KEY not set"))?;

        let mut calldata = Self::cancel_market_selector().to_vec();
        calldata.extend_from_slice(&encode(&[Token::Uint(U256::from(market_id))]));

        let nonce = self
            .provider
            .get_transaction_count(wallet.address(), None)
            .await?;
        let gas_price = self.provider.get_gas_price().await?;

        let tx = TransactionRequest::new()
            .to(self.contract_address)
            .data(calldata)
            .nonce(nonce)
            .gas_price(gas_price)
            .gas(200_000u64)
            .chain_id(self.chain_id);

        let signed = wallet.sign_transaction(&tx.clone().into()).await?;
        let raw = tx.rlp_signed(&signed);
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
    ) -> Result<ContractOrder> {
        let to_addr: Address = to
            .parse()
            .map_err(|_| anyhow::anyhow!("invalid address: {}", to))?;
        Ok(ContractOrder {
            market_id: U256::from(market_id),
            outcome,
            to: to_addr,
            shares: U256::from(shares),
            cost: U256::from(cost),
            deadline: U256::from(deadline),
            nonce: U256::from(nonce),
        })
    }
}
