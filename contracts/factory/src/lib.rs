use kite_types::{Asset, DepositState, Transfer, DEPOSIT_RESERVE};
use near_sdk::json_types::{Base58CryptoHash, U128};
use near_sdk::serde_json::json;
use near_sdk::store::LookupSet;
use near_sdk::{
    env, ext_contract, near, require, AccountId, BorshStorageKey, CryptoHash, Gas, NearToken,
    PanicOnDefault, Promise, PromiseOrValue,
};

pub const REGISTRATION_DEPOSIT: NearToken = NearToken::from_micronear(2_500);
const GAS_REGISTER: Gas = Gas::from_tgas(10);
const GAS_CALLBACK: Gas = Gas::from_tgas(10);
const GAS_FUND: Gas = Gas::from_tgas(5);

#[ext_contract(ext_deposit)]
#[allow(dead_code)]
trait Kite {
    fn settle(&mut self);
    fn refund(&mut self);
    fn recover(&mut self, token: Option<AccountId>, destination: Option<AccountId>);
    fn close(&mut self, to: AccountId) -> U128;
}

#[near(event_json(standard = "kite"))]
pub enum Event {
    #[event_version("1.0.0")]
    Deployed {
        transfer_id: String,
        kite: AccountId,
    },
}

#[derive(BorshStorageKey)]
#[near]
enum Key {
    Operators,
    Implementations,
}

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct KiteFactory {
    admin: AccountId,
    operators: LookupSet<AccountId>,
    implementations: LookupSet<CryptoHash>,
    implementation: CryptoHash,
}

#[near]
impl KiteFactory {
    #[init]
    pub fn new(
        admin: AccountId,
        operator: Option<AccountId>,
        implementation: Base58CryptoHash,
    ) -> Self {
        let mut this = Self {
            admin,
            operators: LookupSet::new(Key::Operators),
            implementations: LookupSet::new(Key::Implementations),
            implementation: implementation.into(),
        };

        if let Some(operator) = operator {
            this.operators.insert(operator);
        }

        this.implementations.insert(implementation.into());
        this
    }

    pub fn admin(&self) -> AccountId {
        self.admin.clone()
    }

    pub fn is_operator(&self, account_id: AccountId) -> bool {
        self.operators.contains(&account_id)
    }

    pub fn implementation(&self) -> Base58CryptoHash {
        self.implementation.into()
    }

    pub fn is_implementation(&self, implementation: Base58CryptoHash) -> bool {
        self.implementations
            .contains(&CryptoHash::from(implementation))
    }

    pub fn predict_address(&self, transfer: Transfer) -> AccountId {
        self.state_for(transfer).account_id()
    }

    pub fn deploy_deposit(&self, transfer: Transfer) -> U128 {
        U128(self.required_deposit(&transfer).as_yoctonear())
    }

    pub fn register_deposit(&self, transfer: Transfer) -> U128 {
        U128(match transfer.pay_in {
            Asset::Native => 0,
            Asset::Token(_) => REGISTRATION_DEPOSIT.as_yoctonear(),
        })
    }

    pub fn set_implementation(&mut self, implementation: Base58CryptoHash) {
        self.assert_admin();
        let hash: CryptoHash = implementation.into();
        self.implementations.insert(hash);
        self.implementation = hash;
    }

    pub fn grant_operator(&mut self, account_id: AccountId) {
        self.assert_admin();
        self.operators.insert(account_id);
    }

    pub fn revoke_operator(&mut self, account_id: AccountId) {
        self.assert_admin();
        self.operators.remove(&account_id);
    }

    pub fn set_admin(&mut self, account_id: AccountId) {
        self.assert_admin();
        self.admin = account_id;
    }

    pub fn withdraw(&mut self, to: AccountId, amount: U128) -> Promise {
        self.assert_admin();
        Promise::new(to).transfer(NearToken::from_yoctonear(amount.0))
    }

    pub fn upgrade(&mut self) -> Promise {
        self.assert_admin();
        let code = env::input().unwrap_or_else(|| env::panic_str("InvalidConfig: no code"));
        Promise::new(env::current_account_id()).deploy_contract(code)
    }

    #[payable]
    pub fn register(&mut self, transfer: Transfer) -> Promise {
        self.assert_operator();
        let Asset::Token(token) = transfer.pay_in.clone() else {
            env::panic_str("InvalidConfig: native NEAR needs no registration");
        };

        let attached = env::attached_deposit();
        require!(
            !attached.is_zero(),
            format!("InsufficientDeposit: {REGISTRATION_DEPOSIT}")
        );

        let account_id = self.state_for(transfer).account_id();
        let registration = attached.min(REGISTRATION_DEPOSIT);
        let change = attached.saturating_sub(registration);

        Promise::new(token)
            .function_call(
                "storage_deposit",
                json!({ "account_id": account_id, "registration_only": true }).to_string(),
                registration,
                GAS_REGISTER,
            )
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(GAS_CALLBACK)
                    .on_registered(
                        account_id,
                        env::predecessor_account_id(),
                        U128(change.as_yoctonear()),
                    ),
            )
    }

    #[payable]
    pub fn deploy(&mut self, transfer: Transfer) -> Promise {
        self.assert_operator();
        let required = self.required_deposit(&transfer);
        let attached = env::attached_deposit();

        require!(
            attached >= required,
            format!("InsufficientDeposit: {required}")
        );

        let tokens = transfer.tokens_to_register();
        let transfer_id = transfer.id.clone();
        let state = self.state_for(transfer);
        let account_id = state.account_id();

        let mut promise = Promise::new(account_id.clone())
            .state_init(state.state_init(), state.state_init_deposit())
            .function_call("fund".to_string(), vec![], DEPOSIT_RESERVE, GAS_FUND);

        for token in tokens {
            promise = promise.and(Promise::new(token).function_call(
                "storage_deposit",
                json!({ "account_id": account_id, "registration_only": true }).to_string(),
                REGISTRATION_DEPOSIT,
                GAS_REGISTER,
            ));
        }

        let change = attached.saturating_sub(required);
        promise.then(
            Self::ext(env::current_account_id())
                .with_static_gas(GAS_CALLBACK)
                .on_deployed(
                    transfer_id,
                    account_id,
                    env::predecessor_account_id(),
                    U128(change.as_yoctonear()),
                ),
        )
    }

    pub fn settle(&mut self, transfer: Transfer) -> Promise {
        self.assert_operator();
        let account_id = self.state_for(transfer).account_id();
        ext_deposit::ext(account_id)
            .with_unused_gas_weight(1)
            .settle()
    }

    pub fn refund(&mut self, transfer: Transfer) -> Promise {
        if env::block_timestamp() / 1_000_000_000 <= transfer.expires_at.0 {
            self.assert_operator();
        }

        let account_id = self.state_for(transfer).account_id();
        ext_deposit::ext(account_id)
            .with_unused_gas_weight(1)
            .refund()
    }

    pub fn recover(
        &mut self,
        transfer: Transfer,
        token: Option<AccountId>,
        destination: Option<AccountId>,
    ) -> Promise {
        self.assert_operator();
        let account_id = self.state_for(transfer).account_id();
        ext_deposit::ext(account_id)
            .with_unused_gas_weight(1)
            .recover(token, destination)
    }

    pub fn close(&mut self, transfer: Transfer) -> Promise {
        self.assert_operator();
        let account_id = self.state_for(transfer).account_id();
        ext_deposit::ext(account_id)
            .with_unused_gas_weight(1)
            .close(env::predecessor_account_id())
    }

    #[private]
    pub fn on_registered(
        &mut self,
        account_id: AccountId,
        payer: AccountId,
        change: U128,
    ) -> AccountId {
        require!(
            env::promise_result_checked(0, 256).is_ok(),
            "RegistrationFailed"
        );

        if change.0 > 0 {
            Promise::new(payer)
                .transfer(NearToken::from_yoctonear(change.0))
                .detach();
        }

        account_id
    }

    #[private]
    pub fn on_deployed(
        &mut self,
        transfer_id: String,
        account_id: AccountId,
        payer: AccountId,
        change: U128,
    ) -> PromiseOrValue<AccountId> {
        let created = env::promise_result_checked(0, 64).is_ok();
        let registered =
            (1..env::promise_results_count()).all(|i| env::promise_result_checked(i, 256).is_ok());

        require!(created && registered, "DeployFailed");
        Event::Deployed {
            transfer_id,
            kite: account_id.clone(),
        }
        .emit();

        if change.0 > 0 {
            Promise::new(payer)
                .transfer(NearToken::from_yoctonear(change.0))
                .detach();
        }

        PromiseOrValue::Value(account_id)
    }
}

impl KiteFactory {
    fn assert_admin(&self) {
        require!(env::predecessor_account_id() == self.admin, "OnlyAdmin");
    }

    fn assert_operator(&self) {
        require!(
            self.operators.contains(&env::predecessor_account_id()),
            "OnlyOperator"
        );
    }

    fn required_deposit(&self, transfer: &Transfer) -> NearToken {
        let registrations = transfer.tokens_to_register().len() as u128;
        self.state_for(transfer.clone())
            .state_init_deposit()
            .saturating_add(DEPOSIT_RESERVE)
            .saturating_add(REGISTRATION_DEPOSIT.saturating_mul(registrations))
    }

    fn state_for(&self, transfer: Transfer) -> DepositState {
        if let Err(e) = transfer.validate() {
            env::panic_str(e);
        }

        if !self
            .implementations
            .contains(&CryptoHash::from(transfer.implementation))
        {
            env::panic_str(&format!(
                "UnknownImplementation: {}",
                String::from(&transfer.implementation)
            ));
        }

        DepositState::initial(env::current_account_id(), transfer)
    }
}

#[cfg(test)]
mod tests;
