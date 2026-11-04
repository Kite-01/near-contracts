#![allow(unused_must_use)]

use super::*;
use kite_types::{Fee, Swap};
use near_sdk::json_types::Base58CryptoHash;
use near_sdk::test_utils::{get_logs, VMContextBuilder};
use near_sdk::{testing_env, PromiseResult, RuntimeFeesConfig};

const NOW_S: u64 = 1_700_000_000;

fn factory() -> AccountId {
    "factory.near".parse().unwrap()
}

fn me() -> AccountId {
    "0s0123456789abcdef0123456789abcdef01234567"
        .parse()
        .unwrap()
}

fn usdc() -> AccountId {
    "usdc.near".parse().unwrap()
}

fn transfer(pay_in: Asset, settlement: Asset) -> Transfer {
    Transfer {
        id: "t-1".into(),
        pay_in: pay_in.clone(),
        amount_in: U128(1_000),
        swap: (pay_in != settlement).then(|| Swap {
            router: "v2.ref-finance.near".parse().unwrap(),
            pool: SwapPool::RefV1(U64(4)),
            wrap: "wrap.near".parse().unwrap(),
        }),
        settlement_token: settlement,
        settlement_recipient: "shop.near".parse().unwrap(),
        min_amount_out: U128(900),
        fees: vec![
            Fee {
                id: "f-1".into(),
                recipient: "kite.near".parse().unwrap(),
                amount: U128(10),
            },
            Fee {
                id: "f-2".into(),
                recipient: "app.near".parse().unwrap(),
                amount: U128(20),
            },
        ],
        refund_account: "payer.near".parse().unwrap(),
        expires_at: U64(NOW_S + 3600),
        implementation: Base58CryptoHash::from([1u8; 32]),
    }
}

fn contract(t: Transfer) -> Kite {
    let mut s = DepositState::initial(factory(), t);
    s.status = Status::Open;
    Kite { s }
}

fn ctx(predecessor: AccountId, balance: u128, now_s: u64, results: Vec<PromiseResult>) {
    let mut b = VMContextBuilder::new();
    b.current_account_id(me())
        .predecessor_account_id(predecessor)
        .account_balance(NearToken::from_yoctonear(balance))
        .block_timestamp(now_s * 1_000_000_000)
        .prepaid_gas(Gas::from_tgas(300))
        .storage_usage(434);

    testing_env!(
        b.build(),
        near_sdk::test_vm_config(),
        RuntimeFeesConfig::test(),
        Default::default(),
        results
    );
}

fn reserve() -> u128 {
    DEPOSIT_RESERVE.as_yoctonear()
}

fn ok() -> PromiseResult {
    PromiseResult::Successful(vec![])
}

fn ok_u128(v: u128) -> PromiseResult {
    PromiseResult::Successful(near_sdk::serde_json::to_vec(&U128(v)).unwrap())
}

fn events(name: &str) -> usize {
    get_logs()
        .iter()
        .filter(|l| l.contains(&format!("\"event\":\"{name}\"")))
        .count()
}

#[test]
#[should_panic(expected = "OnlyFactory")]
fn settle_only_factory() {
    ctx("eve.near".parse().unwrap(), reserve(), NOW_S, vec![]);
    contract(transfer(Asset::Native, Asset::Native)).settle();
}

#[test]
#[should_panic(expected = "Expired")]
fn settle_after_expiry() {
    ctx(factory(), reserve() + 1_000, NOW_S + 7200, vec![]);
    contract(transfer(Asset::Native, Asset::Native)).settle();
}

#[test]
fn settle_native_underpaid_stays_open() {
    ctx(factory(), reserve() + 999, NOW_S, vec![]);
    let mut c = contract(transfer(Asset::Native, Asset::Native));
    c.settle();
    assert_eq!(c.get_status(), Status::Open);
    assert_eq!(events("settle_stopped"), 1);
}

#[test]
fn settle_native_pays_fees_and_recipient() {
    ctx(factory(), reserve() + 1_050, NOW_S, vec![]);
    let mut c = contract(transfer(Asset::Native, Asset::Native));
    c.settle();
    assert_eq!(c.get_status(), Status::Executing);
    let p = c.get_progress();
    assert_eq!(p.step, Step::Pay);
    assert_eq!(p.excess, U128(50));
    assert_eq!(p.amount_out, U128(1_000));

    ctx(me(), reserve() + 50, NOW_S, vec![ok(), ok(), ok()]);
    c.on_paid(vec![1, 2, RECIPIENT_BIT]);
    assert_eq!(c.get_status(), Status::Settled);
    assert_eq!(events("fee_paid"), 2);
    assert_eq!(events("settled"), 1);
}

#[test]
fn failed_payout_resumes_with_the_rest() {
    ctx(factory(), reserve() + 1_000, NOW_S, vec![]);
    let mut c = contract(transfer(Asset::Native, Asset::Native));
    c.settle();

    ctx(
        me(),
        reserve(),
        NOW_S,
        vec![ok(), PromiseResult::Failed, ok()],
    );

    c.on_paid(vec![1, 2, RECIPIENT_BIT]);
    assert_eq!(c.get_status(), Status::Executing);
    assert_eq!(c.get_progress().paid, 1 | RECIPIENT_BIT);
    assert_eq!(c.get_progress().lock_until, U64(0));

    ctx(factory(), reserve(), NOW_S, vec![]);
    c.settle();
    ctx(me(), reserve(), NOW_S, vec![ok()]);
    c.on_paid(vec![2]);
    assert_eq!(c.get_status(), Status::Settled);
}

#[test]
#[should_panic(expected = "Busy")]
fn settle_in_flight_is_locked() {
    ctx(factory(), reserve() + 1_000, NOW_S, vec![]);
    let mut c = contract(transfer(Asset::Native, Asset::Native));
    c.settle();
    c.settle();
}

#[test]
fn token_settle_reads_balance_then_pays() {
    ctx(factory(), reserve(), NOW_S, vec![]);
    let mut c = contract(transfer(Asset::Token(usdc()), Asset::Token(usdc())));
    c.settle();
    assert_eq!(c.get_progress().step, Step::CheckBalance);

    ctx(me(), reserve(), NOW_S, vec![ok_u128(999)]);
    c.on_balance(Ok(U128(999)));
    assert_eq!(c.get_status(), Status::Open);

    ctx(factory(), reserve(), NOW_S, vec![]);
    c.settle();
    ctx(me(), reserve(), NOW_S, vec![ok_u128(1_200)]);
    c.on_balance(Ok(U128(1_200)));
    assert_eq!(c.get_progress().step, Step::Pay);
    assert_eq!(c.get_progress().excess, U128(200));
}

#[test]
fn swap_failure_reopens_and_success_reads_output() {
    let usdt: AccountId = "usdt.near".parse().unwrap();
    ctx(factory(), reserve(), NOW_S, vec![]);
    let mut c = contract(transfer(Asset::Token(usdt), Asset::Token(usdc())));
    c.settle();
    ctx(me(), reserve(), NOW_S, vec![]);
    c.on_balance(Ok(U128(1_000)));
    assert_eq!(c.get_progress().step, Step::ReadOutBefore);
    c.on_out_before(Ok(U128(5)));
    assert_eq!(c.get_progress().step, Step::Swap);
    c.on_swapped(Ok(U128(0)));
    assert_eq!(c.get_status(), Status::Open);

    ctx(factory(), reserve(), NOW_S, vec![]);
    c.settle();
    ctx(me(), reserve(), NOW_S, vec![]);
    c.on_balance(Ok(U128(1_000)));
    c.on_out_before(Ok(U128(5)));
    c.on_swapped(Ok(U128(1_000)));
    assert_eq!(c.get_progress().step, Step::ReadOutAfter);
    c.on_out_after(Ok(U128(800)));
    assert_eq!(
        c.get_status(),
        Status::Executing,
        "below min stays executing"
    );
    c.on_out_after(Ok(U128(955)));
    assert_eq!(c.get_progress().amount_out, U128(950));
    assert_eq!(c.get_progress().step, Step::Pay);
}

#[test]
fn native_swap_wraps_once() {
    ctx(factory(), reserve() + 1_000, NOW_S, vec![]);
    let mut c = contract(transfer(Asset::Native, Asset::Token(usdc())));
    c.settle();
    assert_eq!(c.get_progress().step, Step::ReadOutBefore);
    ctx(me(), reserve() + 1_000, NOW_S, vec![]);
    c.on_out_before(Ok(U128(0)));
    assert_eq!(c.get_progress().step, Step::Wrap);
    c.on_wrapped(Ok(()));
    assert!(c.get_progress().wrapped);
    c.on_swapped(Err(PromiseError::Failed));
    assert_eq!(c.get_status(), Status::Open);
    assert!(c.get_progress().wrapped);

    ctx(factory(), reserve(), NOW_S, vec![]);
    c.settle();
    assert_eq!(c.get_progress().step, Step::ReadOutBefore);
    ctx(me(), reserve(), NOW_S, vec![]);
    c.on_out_before(Ok(U128(0)));
    assert_eq!(c.get_progress().step, Step::Swap);
}

#[test]
#[should_panic(expected = "NotExpired")]
fn refund_before_expiry_needs_factory() {
    ctx("eve.near".parse().unwrap(), reserve(), NOW_S, vec![]);
    contract(transfer(Asset::Native, Asset::Native)).refund();
}

#[test]
fn refund_after_expiry_by_anyone() {
    ctx(
        "eve.near".parse().unwrap(),
        reserve() + 700,
        NOW_S + 7200,
        vec![],
    );
    let mut c = contract(transfer(Asset::Native, Asset::Native));
    c.refund();
    assert_eq!(c.get_status(), Status::Refunded);
    ctx(me(), reserve(), NOW_S + 7200, vec![ok()]);
    c.on_refunded(U128(700), Ok(()));
    assert!(get_logs()[0].contains("\"amount_refunded\":\"700\""));
}

#[test]
fn refund_to_an_account_that_cant_take_it_reports_zero() {
    ctx(factory(), reserve(), NOW_S, vec![]);
    let mut c = contract(transfer(Asset::Token(usdc()), Asset::Token(usdc())));
    c.refund();
    ctx(me(), reserve(), NOW_S, vec![]);
    c.on_refunded(U128(700), Err(PromiseError::Failed));
    assert!(get_logs()[0].contains("\"amount_refunded\":\"0\""));
}

#[test]
#[should_panic(expected = "InvoiceNotOpen")]
fn refund_twice() {
    ctx(factory(), reserve(), NOW_S, vec![]);
    let mut c = contract(transfer(Asset::Native, Asset::Native));
    c.refund();
    c.refund();
}

#[test]
#[should_panic(expected = "RecoveryNotAllowed")]
fn recover_pay_in_while_open() {
    ctx(factory(), reserve(), NOW_S, vec![]);
    contract(transfer(Asset::Token(usdc()), Asset::Token(usdc()))).recover(Some(usdc()), None);
}

#[test]
#[should_panic(expected = "InvoiceNotOpen")]
fn recover_while_executing() {
    ctx(factory(), reserve() + 1_000, NOW_S, vec![]);
    let mut c = contract(transfer(Asset::Native, Asset::Native));
    c.settle();
    c.recover(Some("other.near".parse().unwrap()), None);
}

#[test]
fn recover_other_token_while_open() {
    ctx(factory(), reserve(), NOW_S, vec![]);
    let mut c = contract(transfer(Asset::Token(usdc()), Asset::Token(usdc())));
    c.recover(
        Some("other.near".parse().unwrap()),
        Some("ops.near".parse().unwrap()),
    );
}

#[test]
fn ft_on_transfer_takes_only_the_pay_in_token() {
    ctx(usdc(), reserve(), NOW_S, vec![]);
    let mut c = contract(transfer(Asset::Token(usdc()), Asset::Token(usdc())));
    let kept = c.ft_on_transfer("payer.near".parse().unwrap(), U128(5), String::new());
    assert!(matches!(kept, PromiseOrValue::Value(U128(0))));
    ctx("other.near".parse().unwrap(), reserve(), NOW_S, vec![]);
    let back = c.ft_on_transfer("payer.near".parse().unwrap(), U128(5), String::new());
    assert!(matches!(back, PromiseOrValue::Value(U128(5))));
}

#[test]
fn a_large_state_keeps_its_storage_stake_out_of_the_payment() {
    let mut b = VMContextBuilder::new();
    let stake = 1_131u128 * 10u128.pow(19);
    b.current_account_id(me())
        .predecessor_account_id(factory())
        .account_balance(NearToken::from_yoctonear(reserve() + stake + 999))
        .block_timestamp(NOW_S * 1_000_000_000)
        .prepaid_gas(Gas::from_tgas(300))
        .storage_usage(1_131);
    testing_env!(b.build());

    let mut c = contract(transfer(Asset::Native, Asset::Native));
    c.settle();
    assert_eq!(c.get_status(), Status::Open, "999 is short of 1_000");
}

fn ctx_paying(predecessor: AccountId, balance: u128, deposit: NearToken) {
    let mut b = VMContextBuilder::new();
    b.current_account_id(me())
        .predecessor_account_id(predecessor)
        .account_balance(NearToken::from_yoctonear(balance))
        .attached_deposit(deposit)
        .block_timestamp(NOW_S * 1_000_000_000)
        .prepaid_gas(Gas::from_tgas(300))
        .storage_usage(434);
    testing_env!(b.build());
}

#[test]
fn fund_takes_the_reserve_once() {
    ctx_paying(factory(), reserve(), DEPOSIT_RESERVE);
    let mut c = Kite {
        s: DepositState::initial(factory(), transfer(Asset::Native, Asset::Native)),
    };
    assert_eq!(c.get_status(), Status::Uninitialized);
    assert!(matches!(c.fund(), PromiseOrValue::Value(())));
    assert_eq!(c.get_status(), Status::Open);
    assert!(matches!(c.fund(), PromiseOrValue::Promise(_)));
}

#[test]
#[should_panic(expected = "OnlyFactory")]
fn fund_only_factory() {
    ctx_paying("eve.near".parse().unwrap(), 0, DEPOSIT_RESERVE);
    contract(transfer(Asset::Native, Asset::Native)).fund();
}

#[test]
#[should_panic(expected = "NotFinished")]
fn close_waits_for_the_transfer_to_finish() {
    ctx(factory(), reserve(), NOW_S, vec![]);
    contract(transfer(Asset::Native, Asset::Native)).close("operator.near".parse().unwrap());
}

#[test]
fn close_returns_the_reserve_but_not_what_is_owed() {
    let mut c = contract(transfer(Asset::Native, Asset::Native));
    c.s.status = Status::Settled;

    ctx(factory(), reserve(), NOW_S, vec![]);
    let PromiseOrValue::Value(swept) = c.close("operator.near".parse().unwrap()) else {
        panic!("expected a value");
    };
    assert_eq!(swept.0, reserve() - CLOSE_FLOOR.as_yoctonear());
    assert_eq!(events("closed"), 1);

    ctx(factory(), reserve() + 500, NOW_S, vec![]);
    let PromiseOrValue::Value(swept) = c.close("operator.near".parse().unwrap()) else {
        panic!("expected a value");
    };

    assert_eq!(swept.0, reserve() - CLOSE_FLOOR.as_yoctonear());
}

#[test]
fn close_unregisters_from_the_tokens_first() {
    let mut c = contract(transfer(Asset::Token(usdc()), Asset::Token(usdc())));
    c.s.status = Status::Refunded;
    ctx(factory(), reserve(), NOW_S, vec![]);
    assert!(matches!(
        c.close("operator.near".parse().unwrap()),
        PromiseOrValue::Promise(_)
    ));

    assert_eq!(events("closed"), 0);
}
