use futures::stream::BoxStream;

use crate::{
    VaultResult,
    object::{Object, ObjectId},
};
use bytes::Bytes;

pub mod command;
pub mod fs;
pub mod http;

pub type ByteStream = BoxStream<'static, VaultResult<Bytes>>;
#[async_trait::async_trait]
pub trait Origin {
    async fn fetch(&self, id: &ObjectId) -> VaultResult<ByteStream>;
    async fn list(&self, id: &ObjectId) -> VaultResult<Vec<Object>>;
    async fn get(&self, id: &ObjectId) -> VaultResult<Object>;
    async fn put(&self, object: &Object) -> VaultResult<()>;
    async fn send(&self, object: &Object, payload: ByteStream) -> VaultResult<()>;
    async fn delete(&self, id: &ObjectId) -> VaultResult<()>;
}
