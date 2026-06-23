use std::num::NonZeroUsize;
use std::str::FromStr;
use std::sync::Arc;

use lru::LruCache;
use parking_lot::Mutex;
use tycho_types::cell::HashBytes;
use tycho_types::models::StdAddr;

use crate::models::sqlx::*;
use crate::sqlx_client::*;
use crate::utils::token_wallets::models::TokenWalletVersion;

#[derive(Clone)]
/// Maps token wallet address to Owner info
pub struct OwnersCache {
    cache: Arc<Mutex<LruCache<StdAddr, OwnerInfo>>>,
    db: SqlxClient,
}

impl OwnersCache {
    pub async fn get(&self, address: &StdAddr) -> Option<OwnerInfo> {
        let info = {
            let mut lock = self.cache.lock();
            lock.get(address).cloned()
        };
        let info = match info {
            Some(a) => a,
            None => {
                let got = self
                    .db
                    .get_token_owner_by_address(address.to_string())
                    .await
                    .ok()?;
                let (_, got) = parse_owner_cache_entry(got)?;
                self.cache.lock().put(address.clone(), got.clone());
                got
            }
        };
        Some(info)
    }
    pub async fn insert(&self, key: StdAddr, value: OwnerInfo) {
        {
            self.cache.lock().put(key.clone(), value.clone());
        }
        let owner = TokenOwnerFromDb {
            address: key.to_string(),
            owner_account_workchain_id: value.owner_address.workchain as i32,
            owner_account_hex: value.owner_address.address.to_string(),
            root_address: value.root_address.to_string(),
            code_hash: value.code_hash.as_array().to_vec(),
            created_at: chrono::Utc::now().naive_utc(), //doesn't matter
            version: value.version.into(),
        };
        if let Err(e) = self.db.new_token_owner(&owner).await {
            tracing::error!("Failed inserting owner info: {}", e)
        }
    }
}

#[derive(Clone, Debug)]
pub struct OwnerInfo {
    pub owner_address: StdAddr,
    pub root_address: StdAddr,
    pub code_hash: HashBytes,
    pub version: TokenWalletVersion,
}

impl OwnersCache {
    pub async fn new(sqlx_client: SqlxClient) -> Result<Self, anyhow::Error> {
        let balances = sqlx_client.get_all_token_owners().await?;
        // no more than 10 mb
        let capacity = NonZeroUsize::new(5000)
            .ok_or_else(|| anyhow::anyhow!("owners cache capacity must be non-zero"))?;
        let mut cache = LruCache::new(capacity);
        balances
            .into_iter()
            .filter_map(parse_owner_cache_entry)
            .for_each(|(key, value)| {
                cache.put(key, value);
            });
        Ok(Self {
            cache: Arc::new(Mutex::new(cache)),
            db: sqlx_client,
        })
    }
}

fn parse_owner_cache_entry(row: TokenOwnerFromDb) -> Option<(StdAddr, OwnerInfo)> {
    let key = parse_std_address(&row.address, "address", &row.address)?;
    let workchain_id = row.owner_account_workchain_id;
    let hex = &row.owner_account_hex;
    let owner = format!("{workchain_id}:{hex}");
    let owner_address = parse_std_address(&owner, "owner_address", &row.address)?;
    let root_address = parse_std_address(&row.root_address, "root_address", &row.address)?;
    let code_hash = parse_code_hash(&row.code_hash, &row.address)?;

    Some((
        key,
        OwnerInfo {
            owner_address,
            root_address,
            code_hash,
            version: row.version.into(),
        },
    ))
}

fn parse_std_address(address: &str, field: &'static str, row_address: &str) -> Option<StdAddr> {
    StdAddr::from_str(address)
        .map_err(|error| {
            tracing::error!(
                row_address,
                field,
                address,
                ?error,
                "failed to parse owner cache address"
            );
        })
        .ok()
}

fn parse_code_hash(code_hash: &[u8], row_address: &str) -> Option<HashBytes> {
    let code_hash: [u8; 32] = code_hash
        .try_into()
        .map_err(|error| {
            tracing::error!(
                row_address,
                len = code_hash.len(),
                ?error,
                "failed to parse owner cache code hash"
            );
        })
        .ok()?;

    Some(HashBytes::from(code_hash))
}
