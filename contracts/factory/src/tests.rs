#![allow(unused_must_use)]

use super::*;
use kite_types::{Asset, Fee, Swap, SwapPool};
use near_sdk::json_types::{U128, U64};
use near_sdk::test_utils::{get_created_receipts, VMContextBuilder};
use near_sdk::testing_env;

const NOW_S: u64 = 1_700_000_000;

fn admin() -> AccountId {
    "admin.near".parse().unwrap()
}

fn operator() -> AccountId {
    "operator.near".parse().unwrap()
}

fn hash(b: u8) -> Base58CryptoHash {
    Base58CryptoHash::from([b; 32])
}

fn transfer() -> Transfer {
    Transfer {
        id: "t-1".into(),
        pay_in: Asset::Native,
        amount_in: U128(1_000),
        settlement_token: Asset::Token("usdc.near".parse().unwrap()),
        settlement_recipient: "shop.near".parse().unwrap(),
        min_amount_out: U128(900),
        fees: vec![Fee {
            id: "f".into(),
            recipient: "kite.near".parse().unwrap(),
            amount: U128(1),
        }],
        refund_account: "payer.near".parse().unwrap(),
        expires_at: U64(NOW_S + 60),
        swap: Some(Swap {
            router: "dclv2.ref-labs.near".parse().unwrap(),
            pool: SwapPool::Dcl("usdc.near|wrap.near|100".into()),
            wrap: "wrap.near".parse().unwrap(),
        }),
        implementation: hash(1),
    }
}

fn ctx(predecessor: AccountId, deposit: NearToken, now_s: u64) {
    let mut b = VMContextBuilder::new();
    b.current_account_id("factory.near".parse().unwrap())
        .predecessor_account_id(predecessor)
        .attached_deposit(deposit)
        .block_timestamp(now_s * 1_000_000_000)
        .prepaid_gas(Gas::from_tgas(300));
    testing_env!(b.build());
}

fn factory() -> KiteFactory {
    ctx(admin(), NearToken::from_yoctonear(0), NOW_S);
    KiteFactory::new(admin(), Some(operator()), hash(1))
}

#[test]
fn predicts_the_state_init_account() {
    let f = factory();
    let expected = DepositState::initial("factory.near".parse().unwrap(), transfer()).account_id();
    assert_eq!(f.predict_address(transfer()), expected);
}

#[test]
#[should_panic(expected = "UnknownImplementation")]
fn rejects_unknown_implementation() {
    let f = factory();
    let mut t = transfer();
    t.implementation = hash(2);
    f.predict_address(t);
}

#[test]
fn old_implementations_keep_working() {
    let mut f = factory();
    f.set_implementation(hash(2));
    assert_eq!(f.implementation(), hash(2));
    assert!(f.is_implementation(hash(1)));
    f.predict_address(transfer());
}

#[test]
#[should_panic(expected = "OnlyAdmin")]
fn only_admin_sets_implementation() {
    let mut f = factory();
    ctx(operator(), NearToken::from_yoctonear(0), NOW_S);
    f.set_implementation(hash(2));
}

#[test]
#[should_panic(expected = "OnlyOperator")]
fn only_operator_deploys() {
    let mut f = factory();
    ctx(admin(), NearToken::from_near(1), NOW_S);
    f.deploy(transfer());
}

#[test]
#[should_panic(expected = "InsufficientDeposit")]
fn deploy_needs_reserve_and_registrations() {
    let mut f = factory();
    let required = f.deploy_deposit(transfer());

    assert_eq!(
        required.0,
        DEPOSIT_RESERVE.as_yoctonear() + 2 * REGISTRATION_DEPOSIT.as_yoctonear()
    );

    ctx(operator(), NearToken::from_yoctonear(required.0 - 1), NOW_S);
    f.deploy(transfer());
}

#[test]
fn deploy_creates_and_registers() {
    let mut f = factory();
    let required = f.deploy_deposit(transfer());

    ctx(operator(), NearToken::from_yoctonear(required.0), NOW_S);
    f.deploy(transfer());

    let receivers: Vec<String> = get_created_receipts()
        .iter()
        .map(|r| r.receiver_id.to_string())
        .collect();

    let predicted = f.predict_address(transfer()).to_string();
    assert!(receivers.contains(&predicted));
    assert!(receivers.contains(&"usdc.near".to_string()));
    assert!(receivers.contains(&"wrap.near".to_string()));
}

#[test]
#[should_panic(expected = "OnlyOperator")]
fn refund_before_expiry_needs_operator() {
    let mut f = factory();

    ctx(
        "eve.near".parse().unwrap(),
        NearToken::from_yoctonear(0),
        NOW_S,
    );

    f.refund(transfer());
}

#[test]
fn refund_after_expiry_by_anyone() {
    let mut f = factory();

    ctx(
        "eve.near".parse().unwrap(),
        NearToken::from_yoctonear(0),
        NOW_S + 61,
    );

    f.refund(transfer());
}

#[test]
#[should_panic(expected = "InvalidConfig")]
fn rejects_invalid_transfers() {
    let f = factory();
    let mut t = transfer();
    t.amount_in = U128(0);
    f.predict_address(t);
}

fn token_transfer() -> Transfer {
    Transfer {
        pay_in: Asset::Token("usdt.near".parse().unwrap()),
        ..transfer()
    }
}

#[test]
fn registers_only_the_pay_in_token() {
    let mut f = factory();
    assert_eq!(f.register_deposit(transfer()).0, 0);

    let required = f.register_deposit(token_transfer());
    assert_eq!(required.0, REGISTRATION_DEPOSIT.as_yoctonear());

    ctx(operator(), NearToken::from_yoctonear(required.0), NOW_S);
    f.register(token_transfer());

    let receivers: Vec<String> = get_created_receipts()
        .iter()
        .map(|r| r.receiver_id.to_string())
        .collect();

    assert!(receivers.contains(&"usdt.near".to_string()));

    let predicted = f.predict_address(token_transfer()).to_string();
    assert!(!receivers.contains(&predicted), "{receivers:?}");
    assert!(!receivers.contains(&"usdc.near".to_string()));
}

#[test]
#[should_panic(expected = "OnlyOperator")]
fn only_operator_registers() {
    let mut f = factory();
    ctx(admin(), REGISTRATION_DEPOSIT, NOW_S);
    f.register(token_transfer());
}

#[test]
#[should_panic(expected = "native NEAR needs no registration")]
fn native_needs_no_registration() {
    let mut f = factory();
    ctx(operator(), REGISTRATION_DEPOSIT, NOW_S);
    f.register(transfer());
}

#[test]
#[should_panic(expected = "InsufficientDeposit")]
fn register_needs_its_deposit() {
    let mut f = factory();
    ctx(operator(), NearToken::from_yoctonear(0), NOW_S);
    f.register(token_transfer());
}

#[test]
#[should_panic(expected = "OnlyOperator")]
fn only_operator_closes() {
    let mut f = factory();
    ctx(
        "eve.near".parse().unwrap(),
        NearToken::from_yoctonear(0),
        NOW_S,
    );
    f.close(transfer());
}
