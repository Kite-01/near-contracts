use kite_types::{
    Asset, DepositState, Progress, Status, Step, SwapPool, Transfer, DEPOSIT_RESERVE,
    RECIPIENT_BIT, STORAGE_MARGIN, ZERO_BALANCE_STORAGE_LIMIT,
};
use near_sdk::json_types::{U128, U64};
use near_sdk::serde_json::json;
use near_sdk::{
    env, ext_contract, near, require, AccountId, Gas, NearToken, PanicOnDefault, Promise,
    PromiseError, PromiseOrValue,
};

const ONE_YOCTO: NearToken = NearToken::from_yoctonear(1);
const GAS_VIEW: Gas = Gas::from_tgas(5);
const GAS_FT_TRANSFER: Gas = Gas::from_tgas(10);
const GAS_WRAP: Gas = Gas::from_tgas(10);
const GAS_SWAP: Gas = Gas::from_tgas(120);
const GAS_CALLBACK: Gas = Gas::from_tgas(20);
const LOCK_NS: u64 = 10 * 60 * 1_000_000_000;
const GAS_UNREGISTER: Gas = Gas::from_tgas(10);
const CLOSE_FLOOR: NearToken = NearToken::from_micronear(1);

#[ext_contract(ext_ft)]
#[allow(dead_code)]
trait FungibleToken {
    fn ft_balance_of(&self, account_id: AccountId) -> U128;
    fn storage_unregister(&mut self, force: Option<bool>) -> bool;
    fn ft_transfer(&mut self, receiver_id: AccountId, amount: U128, memo: Option<String>);
    fn ft_transfer_call(
        &mut self,
        receiver_id: AccountId,
        amount: U128,
        memo: Option<String>,
        msg: String,
    ) -> PromiseOrValue<U128>;
}

#[ext_contract(ext_wrap)]
#[allow(dead_code)]
trait WrappedNear {
    fn near_deposit(&mut self);
    fn near_withdraw(&mut self, amount: U128);
}

#[near(event_json(standard = "kite"))]
pub enum Event {
    #[event_version("1.0.0")]
    Settled {
        transfer_id: String,
        settlement_recipient: AccountId,
        settlement_token: Asset,
        pay_in: Asset,
        amount_in: U128,
        amount_out: U128,
        fee_amount: U128,
    },
    #[event_version("1.0.0")]
    FeePaid {
        transfer_id: String,
        fee_id: String,
        token: Asset,
        recipient: AccountId,
        amount: U128,
    },
    #[event_version("1.0.0")]
    Refunded {
        transfer_id: String,
        refund_account: AccountId,
        pay_in: Asset,
        amount_refunded: U128,
    },
    #[event_version("1.0.0")]
    ExcessRefunded {
        transfer_id: String,
        token: Asset,
        to: AccountId,
        amount: U128,
    },
    #[event_version("1.0.0")]
    Recovered {
        transfer_id: String,
        token: Asset,
        to: AccountId,
        amount: U128,
    },
    #[event_version("1.0.0")]
    Closed {
        transfer_id: String,
        to: AccountId,
        amount: U128,
    },
    #[event_version("1.0.0")]
    SettleStopped {
        transfer_id: String,
        step: Step,
        status: Status,
        reason: String,
    },
}

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct Kite {
    s: DepositState,
}

fn now_s() -> u64 {
    env::block_timestamp() / 1_000_000_000
}

fn gas_left() -> Gas {
    env::prepaid_gas().saturating_sub(env::used_gas())
}

fn send(asset: &Asset, to: &AccountId, amount: u128) -> Promise {
    match asset {
        Asset::Native => Promise::new(to.clone()).transfer(NearToken::from_yoctonear(amount)),
        Asset::Token(token) => ext_ft::ext(token.clone())
            .with_attached_deposit(ONE_YOCTO)
            .with_static_gas(GAS_FT_TRANSFER)
            .ft_transfer(to.clone(), U128(amount), None),
    }
}

fn storage_stake() -> u128 {
    let usage = env::storage_usage();
    if usage <= ZERO_BALANCE_STORAGE_LIMIT {
        0
    } else {
        env::storage_byte_cost()
            .saturating_mul(usage as u128)
            .as_yoctonear()
    }
}

fn balance_of(token: &AccountId) -> Promise {
    ext_ft::ext(token.clone())
        .with_static_gas(GAS_VIEW)
        .ft_balance_of(env::current_account_id())
}

#[near]
impl Kite {
    pub fn get_factory(&self) -> AccountId {
        self.s.factory.clone()
    }

    pub fn get_transfer(&self) -> Transfer {
        self.s.transfer.clone()
    }

    pub fn get_status(&self) -> Status {
        self.s.status
    }

    pub fn get_progress(&self) -> Progress {
        self.s.progress.clone()
    }

    #[payable]
    pub fn fund(&mut self) -> PromiseOrValue<()> {
        self.assert_factory();
        if self.s.status != Status::Uninitialized {
            return PromiseOrValue::Promise(
                Promise::new(self.s.factory.clone()).transfer(env::attached_deposit()),
            );
        }

        self.s.status = Status::Open;
        PromiseOrValue::Value(())
    }

    pub fn close(&mut self, to: AccountId) -> PromiseOrValue<U128> {
        self.assert_factory();
        if !matches!(self.s.status, Status::Settled | Status::Refunded) {
            env::panic_str(&format!("NotFinished: {}", self.s.transfer.id));
        }

        let owed = U128(self.native_available());
        let tokens = self.s.transfer.tokens_to_register();
        let Some(unregister) = tokens
            .into_iter()
            .map(|token| {
                ext_ft::ext(token)
                    .with_attached_deposit(ONE_YOCTO)
                    .with_static_gas(GAS_UNREGISTER)
                    .storage_unregister(None)
            })
            .reduce(|a, b| a.and(b))
        else {
            return PromiseOrValue::Value(self.sweep(to, owed));
        };

        PromiseOrValue::Promise(
            unregister.then(
                Self::ext(env::current_account_id())
                    .with_static_gas(GAS_CALLBACK.saturating_mul(2))
                    .on_unregistered(to, owed),
            ),
        )
    }

    pub fn settle(&mut self) -> PromiseOrValue<()> {
        self.assert_factory();
        match self.s.status {
            Status::Open => {
                if now_s() > self.s.transfer.expires_at.0 {
                    env::panic_str(&format!("Expired: {}", self.s.transfer.id));
                }
                self.s.status = Status::Executing;
                self.s.progress.step = Step::CheckBalance;
            }
            Status::Executing => {
                require!(
                    env::block_timestamp() >= self.s.progress.lock_until.0,
                    "Busy"
                );
            }
            _ => env::panic_str(&format!("InvoiceNotOpen: {}", self.s.transfer.id)),
        }

        self.s.progress.lock_until = U64(env::block_timestamp() + LOCK_NS);
        self.run()
    }

    pub fn refund(&mut self) -> PromiseOrValue<()> {
        let t = &self.s.transfer;
        if env::predecessor_account_id() != self.s.factory && now_s() <= t.expires_at.0 {
            env::panic_str(&format!("NotExpired: {}", t.id));
        }

        if self.s.status != Status::Open {
            env::panic_str(&format!("InvoiceNotOpen: {}", t.id));
        }

        self.s.status = Status::Refunded;

        match t.pay_in.clone() {
            Asset::Native if self.s.progress.wrapped => {
                let wrap = t.swap.as_ref().expect("swap").wrap.clone();
                PromiseOrValue::Promise(
                    ext_wrap::ext(wrap)
                        .with_attached_deposit(ONE_YOCTO)
                        .with_static_gas(GAS_WRAP)
                        .near_withdraw(t.amount_in)
                        .then(
                            Self::ext(env::current_account_id())
                                .with_static_gas(GAS_CALLBACK.saturating_mul(2))
                                .on_refund_unwrapped(),
                        ),
                )
            }
            Asset::Native => self.refund_native(),
            Asset::Token(token) => PromiseOrValue::Promise(
                balance_of(&token).then(
                    Self::ext(env::current_account_id())
                        .with_static_gas(GAS_CALLBACK.saturating_mul(2))
                        .on_refund_balance(),
                ),
            ),
        }
    }

    pub fn recover(
        &mut self,
        token: Option<AccountId>,
        destination: Option<AccountId>,
    ) -> PromiseOrValue<()> {
        self.assert_factory();
        let t = &self.s.transfer;
        if matches!(self.s.status, Status::Uninitialized | Status::Executing) {
            env::panic_str(&format!("InvoiceNotOpen: {}", t.id));
        }

        let asset = token.clone().map(Asset::Token).unwrap_or(Asset::Native);
        let wrapped_input = self.s.progress.wrapped
            && t.swap.as_ref().map(|s| Asset::Token(s.wrap.clone())) == Some(asset.clone());
        if self.s.status == Status::Open
            && (asset == t.pay_in || asset == t.settlement_token || wrapped_input)
        {
            env::panic_str("RecoveryNotAllowed");
        }

        let to = destination.unwrap_or_else(|| t.refund_account.clone());

        match token {
            None => {
                let storage = env::storage_byte_cost()
                    .saturating_mul((env::storage_usage() + STORAGE_MARGIN) as u128)
                    .as_yoctonear();
                let amount = env::account_balance()
                    .as_yoctonear()
                    .saturating_sub(storage);
                if amount == 0 {
                    self.emit_recovered(Asset::Native, to, 0);
                    return PromiseOrValue::Value(());
                }
                PromiseOrValue::Promise(
                    send(&Asset::Native, &to, amount).then(
                        Self::ext(env::current_account_id())
                            .with_static_gas(GAS_CALLBACK)
                            .on_recovered(Asset::Native, to, U128(amount)),
                    ),
                )
            }
            Some(token) => PromiseOrValue::Promise(
                balance_of(&token).then(
                    Self::ext(env::current_account_id())
                        .with_static_gas(GAS_CALLBACK.saturating_mul(2))
                        .on_recover_balance(token, to),
                ),
            ),
        }
    }

    pub fn ft_on_transfer(
        &mut self,
        sender_id: AccountId,
        amount: U128,
        msg: String,
    ) -> PromiseOrValue<U128> {
        let _ = (sender_id, msg);
        let from = env::predecessor_account_id();
        if self.s.status == Status::Open && self.s.transfer.pay_in.token() == Some(&from) {
            PromiseOrValue::Value(U128(0))
        } else {
            PromiseOrValue::Value(amount)
        }
    }

    #[private]
    pub fn on_balance(
        &mut self,
        #[callback_result] r: Result<U128, PromiseError>,
    ) -> PromiseOrValue<()> {
        let Ok(balance) = r else {
            return self.abort("BalanceUnavailable");
        };

        let amount_in = self.s.transfer.amount_in.0;
        if balance.0 < amount_in {
            return self.abort(&format!(
                "InsufficientPayment: {} < {}",
                balance.0, amount_in
            ));
        }

        self.s.progress.excess = U128(balance.0 - amount_in);
        self.after_balance();
        self.run()
    }

    #[private]
    pub fn on_out_before(
        &mut self,
        #[callback_result] r: Result<U128, PromiseError>,
    ) -> PromiseOrValue<()> {
        let Ok(balance) = r else {
            return self.abort("BalanceUnavailable");
        };

        self.s.progress.out_before = balance;
        self.s.progress.step = Step::Wrap;
        self.run()
    }

    #[private]
    pub fn on_wrapped(
        &mut self,
        #[callback_result] r: Result<(), PromiseError>,
    ) -> PromiseOrValue<()> {
        if r.is_err() {
            return self.abort("WrapFailed");
        }

        self.s.progress.wrapped = true;
        self.s.progress.step = Step::Swap;
        self.run()
    }

    #[private]
    pub fn on_swapped(
        &mut self,
        #[callback_result] r: Result<U128, PromiseError>,
    ) -> PromiseOrValue<()> {
        let used = r.map(|u| u.0).unwrap_or(0);
        if used == 0 {
            return self.abort("SwapFailed");
        }

        self.s.progress.step = Step::ReadOutAfter;
        self.run()
    }

    #[private]
    pub fn on_out_after(
        &mut self,
        #[callback_result] r: Result<U128, PromiseError>,
    ) -> PromiseOrValue<()> {
        let Ok(balance) = r else {
            return self.pause("BalanceUnavailable");
        };

        let amount_out = balance.0.saturating_sub(self.s.progress.out_before.0);
        let min = self.s.transfer.min_amount_out.0;
        if amount_out < min {
            return self.pause(&format!("OutputPending: {} < {}", amount_out, min));
        }

        self.s.progress.amount_out = U128(amount_out);
        self.s.progress.step = Step::Unwrap;
        self.run()
    }

    #[private]
    pub fn on_unwrapped(
        &mut self,
        #[callback_result] r: Result<(), PromiseError>,
    ) -> PromiseOrValue<()> {
        if r.is_err() {
            return self.pause("UnwrapFailed");
        }

        self.s.progress.step = Step::Pay;
        self.run()
    }

    #[private]
    pub fn on_paid(&mut self, bits: Vec<u16>) -> PromiseOrValue<()> {
        let t = self.s.transfer.clone();
        for (i, bit) in bits.iter().enumerate() {
            if env::promise_result_checked(i as u64, 64).is_err() {
                continue;
            }

            self.s.progress.paid |= bit;
            if *bit != RECIPIENT_BIT {
                let fee = &t.fees[bit.trailing_zeros() as usize];
                Event::FeePaid {
                    transfer_id: t.id.clone(),
                    fee_id: fee.id.clone(),
                    token: t.settlement_token.clone(),
                    recipient: fee.recipient.clone(),
                    amount: fee.amount,
                }
                .emit();
            }
        }

        if self.s.progress.paid == self.all_paid_bits() {
            self.finish()
        } else {
            self.pause("PayoutFailed")
        }
    }

    #[private]
    pub fn on_excess(
        &mut self,
        to: AccountId,
        amount: U128,
        #[callback_result] r: Result<(), PromiseError>,
    ) {
        if r.is_ok() {
            Event::ExcessRefunded {
                transfer_id: self.s.transfer.id.clone(),
                token: self.s.transfer.pay_in.clone(),
                to,
                amount,
            }
            .emit();
        }
    }

    #[private]
    pub fn on_unregistered(&mut self, to: AccountId, owed: U128) -> Promise {
        Self::ext(env::current_account_id())
            .with_static_gas(GAS_CALLBACK)
            .on_sweep(to, owed)
    }

    #[private]
    pub fn on_sweep(&mut self, to: AccountId, owed: U128) -> U128 {
        self.sweep(to, owed)
    }

    #[private]
    pub fn on_refund_unwrapped(
        &mut self,
        #[callback_result] r: Result<(), PromiseError>,
    ) -> PromiseOrValue<()> {
        if r.is_ok() {
            self.s.progress.wrapped = false;
        }

        self.refund_native()
    }

    #[private]
    pub fn on_refund_balance(
        &mut self,
        #[callback_result] r: Result<U128, PromiseError>,
    ) -> PromiseOrValue<()> {
        let balance = r.map(|b| b.0).unwrap_or(0);
        if balance == 0 {
            self.emit_refunded(0);
            return PromiseOrValue::Value(());
        }

        let t = &self.s.transfer;
        PromiseOrValue::Promise(
            send(&t.pay_in, &t.refund_account, balance).then(
                Self::ext(env::current_account_id())
                    .with_static_gas(GAS_CALLBACK)
                    .on_refunded(U128(balance)),
            ),
        )
    }

    #[private]
    pub fn on_refunded(&mut self, amount: U128, #[callback_result] r: Result<(), PromiseError>) {
        self.emit_refunded(if r.is_ok() { amount.0 } else { 0 });
    }

    #[private]
    pub fn on_recover_balance(
        &mut self,
        token: AccountId,
        to: AccountId,
        #[callback_result] r: Result<U128, PromiseError>,
    ) -> PromiseOrValue<()> {
        let balance = r
            .map(|b| b.0)
            .unwrap_or_else(|_| env::panic_str("BalanceUnavailable"));
        let asset = Asset::Token(token);
        if balance == 0 {
            self.emit_recovered(asset, to, 0);
            return PromiseOrValue::Value(());
        }
        PromiseOrValue::Promise(
            send(&asset, &to, balance).then(
                Self::ext(env::current_account_id())
                    .with_static_gas(GAS_CALLBACK)
                    .on_recovered(asset, to, U128(balance)),
            ),
        )
    }

    #[private]
    pub fn on_recovered(
        &mut self,
        token: Asset,
        to: AccountId,
        amount: U128,
        #[callback_result] r: Result<(), PromiseError>,
    ) {
        if r.is_err() {
            env::panic_str("RecoverFailed");
        }
        self.emit_recovered(token, to, amount.0);
    }
}

impl Kite {
    fn assert_factory(&self) {
        require!(
            env::predecessor_account_id() == self.s.factory,
            "OnlyFactory"
        );
    }

    fn native_available(&self) -> u128 {
        env::account_balance()
            .as_yoctonear()
            .saturating_sub(DEPOSIT_RESERVE.as_yoctonear())
            .saturating_sub(storage_stake())
    }

    fn sweep(&self, to: AccountId, owed: U128) -> U128 {
        let amount = env::account_balance()
            .as_yoctonear()
            .saturating_sub(storage_stake())
            .saturating_sub(CLOSE_FLOOR.as_yoctonear())
            .saturating_sub(owed.0);

        if amount > 0 {
            Promise::new(to.clone())
                .transfer(NearToken::from_yoctonear(amount))
                .detach();
        }

        Event::Closed {
            transfer_id: self.s.transfer.id.clone(),
            to,
            amount: U128(amount),
        }
        .emit();
        U128(amount)
    }

    fn wrap(&self) -> AccountId {
        self.s.transfer.swap.as_ref().expect("swap").wrap.clone()
    }

    fn token_in(&self) -> AccountId {
        self.s
            .transfer
            .pay_in
            .token()
            .cloned()
            .unwrap_or_else(|| self.wrap())
    }

    fn token_out(&self) -> AccountId {
        self.s
            .transfer
            .settlement_token
            .token()
            .cloned()
            .unwrap_or_else(|| self.wrap())
    }

    fn all_paid_bits(&self) -> u16 {
        let fees = self.s.transfer.fees.len() as u32;
        ((1u32 << fees) - 1) as u16 | RECIPIENT_BIT
    }

    fn after_balance(&mut self) {
        if self.s.transfer.needs_swap() {
            self.s.progress.step = Step::ReadOutBefore;
        } else {
            self.s.progress.amount_out = self.s.transfer.amount_in;
            self.s.progress.step = Step::Pay;
        }
    }

    fn cb(&self, gas: Gas) -> KiteExt {
        Self::ext(env::current_account_id())
            .with_static_gas(gas)
            .with_unused_gas_weight(1)
    }

    fn run(&mut self) -> PromiseOrValue<()> {
        loop {
            let t = self.s.transfer.clone();
            let step = self.s.progress.step;

            let calls = match step {
                Step::Wrap => t.pay_in == Asset::Native && !self.s.progress.wrapped,
                Step::Unwrap => t.settlement_token == Asset::Native,
                Step::CheckBalance => t.pay_in != Asset::Native,
                Step::Done => false,
                _ => true,
            };

            let needed = match step {
                Step::Swap => GAS_SWAP.saturating_add(GAS_CALLBACK.saturating_mul(2)),
                Step::Pay => GAS_FT_TRANSFER
                    .saturating_mul(t.fees.len() as u64 + 2)
                    .saturating_add(GAS_CALLBACK.saturating_mul(2)),
                _ => GAS_VIEW.saturating_add(GAS_CALLBACK.saturating_mul(2)),
            };

            if calls && gas_left() < needed {
                return self.pause("OutOfGas");
            }

            match step {
                Step::CheckBalance => match &t.pay_in {
                    Asset::Native if self.s.progress.wrapped => {
                        // a swap that failed after wrapping left the payment as wNEAR.
                        self.after_balance();
                    }
                    Asset::Native => {
                        let balance = self.native_available();
                        if balance < t.amount_in.0 {
                            return self.abort(&format!(
                                "InsufficientPayment: {} < {}",
                                balance, t.amount_in.0
                            ));
                        }

                        self.s.progress.excess = U128(balance - t.amount_in.0);
                        self.after_balance();
                    }
                    Asset::Token(token) => {
                        return PromiseOrValue::Promise(
                            balance_of(token).then(self.cb(GAS_CALLBACK).on_balance()),
                        )
                    }
                },
                Step::ReadOutBefore => {
                    return PromiseOrValue::Promise(
                        balance_of(&self.token_out()).then(self.cb(GAS_CALLBACK).on_out_before()),
                    )
                }
                Step::Wrap => {
                    if t.pay_in == Asset::Native && !self.s.progress.wrapped {
                        return PromiseOrValue::Promise(
                            ext_wrap::ext(self.wrap())
                                .with_attached_deposit(NearToken::from_yoctonear(t.amount_in.0))
                                .with_static_gas(GAS_WRAP)
                                .near_deposit()
                                .then(self.cb(GAS_CALLBACK).on_wrapped()),
                        );
                    }

                    self.s.progress.step = Step::Swap;
                }
                Step::Swap => {
                    let swap = t.swap.clone().expect("swap");
                    let (token_in, token_out) = (self.token_in(), self.token_out());
                    let msg = match &swap.pool {
                        SwapPool::RefV1(pool_id) => json!({
                            "force": 0,
                            "actions": [{
                                "pool_id": pool_id.0,
                                "token_in": token_in,
                                "token_out": token_out,
                                "amount_in": t.amount_in,
                                "min_amount_out": t.min_amount_out,
                            }],
                            "skip_unwrap_near": true,
                        }),
                        SwapPool::Dcl(pool_id) => json!({
                            "Swap": {
                                "pool_ids": [pool_id],
                                "output_token": token_out,
                                "min_output_amount": t.min_amount_out,
                                "skip_unwrap_near": true,
                            }
                        }),
                    };

                    return PromiseOrValue::Promise(
                        ext_ft::ext(token_in)
                            .with_attached_deposit(ONE_YOCTO)
                            .with_static_gas(GAS_SWAP)
                            .ft_transfer_call(swap.router, t.amount_in, None, msg.to_string())
                            .then(self.cb(GAS_CALLBACK).on_swapped()),
                    );
                }
                Step::ReadOutAfter => {
                    return PromiseOrValue::Promise(
                        balance_of(&self.token_out()).then(self.cb(GAS_CALLBACK).on_out_after()),
                    )
                }
                Step::Unwrap => {
                    if t.settlement_token == Asset::Native {
                        return PromiseOrValue::Promise(
                            ext_wrap::ext(self.wrap())
                                .with_attached_deposit(ONE_YOCTO)
                                .with_static_gas(GAS_WRAP)
                                .near_withdraw(self.s.progress.amount_out)
                                .then(self.cb(GAS_CALLBACK).on_unwrapped()),
                        );
                    }

                    self.s.progress.step = Step::Pay;
                }
                Step::Pay => return self.pay(),
                Step::Done => return PromiseOrValue::Value(()),
            }
        }
    }

    fn pay(&mut self) -> PromiseOrValue<()> {
        let t = self.s.transfer.clone();
        let paid = self.s.progress.paid;
        let mut bits: Vec<u16> = Vec::new();
        let mut joint: Option<Promise> = None;
        let mut add = |p: Promise, bit: u16| {
            bits.push(bit);
            joint = Some(match joint.take() {
                Some(j) => j.and(p),
                None => p,
            });
        };

        for (i, fee) in t.fees.iter().enumerate() {
            let bit = 1u16 << i;
            if paid & bit == 0 {
                add(send(&t.settlement_token, &fee.recipient, fee.amount.0), bit);
            }
        }

        if paid & RECIPIENT_BIT == 0 {
            let net = self.s.progress.amount_out.0 - t.total_fees();
            add(
                send(&t.settlement_token, &t.settlement_recipient, net),
                RECIPIENT_BIT,
            );
        }

        match joint {
            None => self.finish(),
            Some(p) => PromiseOrValue::Promise(p.then(self.cb(GAS_CALLBACK).on_paid(bits))),
        }
    }

    fn finish(&mut self) -> PromiseOrValue<()> {
        let t = self.s.transfer.clone();
        self.s.status = Status::Settled;
        self.s.progress.step = Step::Done;
        self.s.progress.lock_until = U64(0);
        Event::Settled {
            transfer_id: t.id.clone(),
            settlement_recipient: t.settlement_recipient.clone(),
            settlement_token: t.settlement_token.clone(),
            pay_in: t.pay_in.clone(),
            amount_in: t.amount_in,
            amount_out: self.s.progress.amount_out,
            fee_amount: U128(t.total_fees()),
        }
        .emit();

        let excess = self.s.progress.excess.0;
        if excess == 0 {
            return PromiseOrValue::Value(());
        }

        PromiseOrValue::Promise(
            send(&t.pay_in, &t.refund_account, excess).then(
                Self::ext(env::current_account_id())
                    .with_static_gas(GAS_CALLBACK)
                    .on_excess(t.refund_account.clone(), U128(excess)),
            ),
        )
    }

    fn abort(&mut self, reason: &str) -> PromiseOrValue<()> {
        self.s.status = Status::Open;
        let step = self.s.progress.step;
        let wrapped = self.s.progress.wrapped;
        let excess = if wrapped {
            self.s.progress.excess
        } else {
            U128(0)
        };

        self.s.progress = Progress {
            wrapped,
            excess,
            ..Progress::default()
        };

        self.stopped(step, reason);
        PromiseOrValue::Value(())
    }

    fn pause(&mut self, reason: &str) -> PromiseOrValue<()> {
        self.s.progress.lock_until = U64(0);
        self.stopped(self.s.progress.step, reason);
        PromiseOrValue::Value(())
    }

    fn stopped(&self, step: Step, reason: &str) {
        Event::SettleStopped {
            transfer_id: self.s.transfer.id.clone(),
            step,
            status: self.s.status,
            reason: reason.to_string(),
        }
        .emit();
    }

    fn refund_native(&mut self) -> PromiseOrValue<()> {
        let balance = self.native_available();
        if balance == 0 {
            self.emit_refunded(0);
            return PromiseOrValue::Value(());
        }

        let to = self.s.transfer.refund_account.clone();
        PromiseOrValue::Promise(
            send(&Asset::Native, &to, balance).then(
                Self::ext(env::current_account_id())
                    .with_static_gas(GAS_CALLBACK)
                    .on_refunded(U128(balance)),
            ),
        )
    }

    fn emit_refunded(&self, amount: u128) {
        let t = &self.s.transfer;
        Event::Refunded {
            transfer_id: t.id.clone(),
            refund_account: t.refund_account.clone(),
            pay_in: t.pay_in.clone(),
            amount_refunded: U128(amount),
        }
        .emit();
    }

    fn emit_recovered(&self, token: Asset, to: AccountId, amount: u128) {
        Event::Recovered {
            transfer_id: self.s.transfer.id.clone(),
            token,
            to,
            amount: U128(amount),
        }
        .emit();
    }
}

#[cfg(test)]
mod tests;
