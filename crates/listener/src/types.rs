use alloy::primitives::{Address, U256};
use serde::{Deserialize, Serialize};
use crate::config::DexType;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolState {
    #[serde(serialize_with = "serialize_address", deserialize_with = "deserialize_address")]
    pub pair_address: Address,
    #[serde(serialize_with = "serialize_address", deserialize_with = "deserialize_address")]
    pub token0: Address,
    #[serde(serialize_with = "serialize_address", deserialize_with = "deserialize_address")]
    pub token1: Address,
    #[serde(serialize_with = "serialize_u256", deserialize_with = "deserialize_u256")]
    pub reserve0: U256,
    #[serde(serialize_with = "serialize_u256", deserialize_with = "deserialize_u256")]
    pub reserve1: U256,
    pub dex_name: String,
    pub dex_type: DexType,
    pub fee_bps: u32,
    pub token0_decimals: u8,
    pub token1_decimals: u8,
    pub last_updated_block: u64,
}

fn serialize_address<S>(addr: &Address, s: S) -> Result<S::Ok, S::Error>
where S: serde::Serializer {
    s.serialize_str(&format!("{:?}", addr))
}

fn deserialize_address<'de, D>(d: D) -> Result<Address, D::Error>
where D: serde::Deserializer<'de> {
    let s = String::deserialize(d)?;
    s.parse::<Address>().map_err(serde::de::Error::custom)
}

fn serialize_u256<S>(val: &U256, s: S) -> Result<S::Ok, S::Error>
where S: serde::Serializer {
    s.serialize_str(&val.to_string())
}

fn deserialize_u256<'de, D>(d: D) -> Result<U256, D::Error>
where D: serde::Deserializer<'de> {
    let s = String::deserialize(d)?;
    s.parse::<U256>().map_err(serde::de::Error::custom)
}
