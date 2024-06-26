//! Contains the [EthereumDataSource], which is a concrete implementation of the
//! [DataAvailabilityProvider] trait for the Ethereum protocol.

use crate::{
    sources::{BlobSource, CalldataSource, EthereumDataSourceVariant},
    traits::{BlobProvider, ChainProvider, DataAvailabilityProvider},
    types::{BlockInfo, RollupConfig},
};
use alloc::{boxed::Box, fmt::Debug};
use alloy_primitives::{Address, Bytes};
use anyhow::Result;
use async_trait::async_trait;

/// A factory for creating an Ethereum data source provider.
#[derive(Debug, Clone, Copy)]
pub struct EthereumDataSource<C, B>
where
    C: ChainProvider + Send + Clone,
    B: BlobProvider + Clone,
{
    /// The chain provider to use for the factory.
    pub chain_provider: C,
    /// The blob provider
    pub blob_provider: B,
    /// The ecotone timestamp.
    pub ecotone_timestamp: Option<u64>,
    /// The L1 Signer.
    pub signer: Address,
}

impl<C, B> EthereumDataSource<C, B>
where
    C: ChainProvider + Send + Clone + Debug,
    B: BlobProvider + Clone + Debug,
{
    /// Creates a new factory.
    pub fn new(provider: C, blobs: B, cfg: &RollupConfig) -> Self {
        Self {
            chain_provider: provider,
            blob_provider: blobs,
            ecotone_timestamp: cfg.ecotone_time,
            signer: cfg.genesis.system_config.batcher_addr,
        }
    }
}

#[async_trait]
impl<C, B> DataAvailabilityProvider for EthereumDataSource<C, B>
where
    C: ChainProvider + Send + Sync + Clone + Debug,
    B: BlobProvider + Send + Sync + Clone + Debug,
{
    type Item = Bytes;
    type DataIter = EthereumDataSourceVariant<C, B>;

    async fn open_data(
        &self,
        block_ref: &BlockInfo,
        batcher_address: Address,
    ) -> Result<Self::DataIter> {
        let ecotone_enabled =
            self.ecotone_timestamp.map(|e| block_ref.timestamp >= e).unwrap_or(false);
        if ecotone_enabled {
            Ok(EthereumDataSourceVariant::Blob(BlobSource::new(
                self.chain_provider.clone(),
                self.blob_provider.clone(),
                batcher_address,
                *block_ref,
                self.signer,
            )))
        } else {
            Ok(EthereumDataSourceVariant::Calldata(CalldataSource::new(
                self.chain_provider.clone(),
                batcher_address,
                *block_ref,
                self.signer,
            )))
        }
    }
}
