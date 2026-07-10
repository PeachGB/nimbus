use std::path::PathBuf;

use vault::{object::ObjectId, vault::Vault};

pub struct App {
    vaults: Vec<Vault>,
    current: ObjectId,
}
impl App {
    pub fn init() -> Self {
        App {
            vaults: vec![],
            current: ObjectId::default(),
        }
    }

    pub fn ls(&self) {
        let names: Vec<String> = self
            .vaults
            .iter()
            .map(|vault| vault.get_name().clone())
            .collect();
        for name in names {
            println!("{}", name);
        }
    }

    pub fn new(&self, cfg: PathBuf) {
        Vault::new(cfg);
    }
    pub async fn cd(&self, path: &str) {
        todo!()
    }
    pub async fn put() {
        todo!()
    }
    pub async fn get() {
        todo!()
    }
    pub async fn cp() {
        todo!()
    }
    pub async fn mv() {
        todo!()
    }
    pub async fn origin() {
        todo!()
    }
    pub async fn sync() {
        todo!()
    }
}
