use crate::backend::Model;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone, Debug)]
pub struct Context {
    pub model: Arc<Mutex<Model>>,
}

impl Context {
    pub async fn new() -> Self {
        Self {
            model: Arc::new(Mutex::new(Model::new())),
        }
    }
}
