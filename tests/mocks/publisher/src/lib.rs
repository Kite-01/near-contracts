use near_sdk::{env, near, Promise};

#[near(contract_state)]
#[derive(Default)]
pub struct Publisher {}

#[near]
impl Publisher {
    pub fn publish(&mut self) -> Promise {
        let code = env::input().unwrap_or_else(|| env::panic_str("no code"));
        Promise::new(env::current_account_id()).deploy_global_contract(code)
    }
}
