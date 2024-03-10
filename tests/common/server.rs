use async_trait::async_trait;

#[async_trait]
/// This Rust code defines a trait named `Server`. The trait has two methods:
pub trait Server {
    async fn stop(self: Box<Self>) -> usize;
    fn address(&self) -> String;
}