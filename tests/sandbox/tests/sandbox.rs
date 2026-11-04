#![allow(unused_must_use)]

use kite_types::{Asset, Fee, Swap, SwapPool, Transfer, DEPOSIT_RESERVE};
use near_workspaces::network::Sandbox;
use near_workspaces::result::ExecutionFinalResult;
use near_workspaces::types::NearToken;
use near_workspaces::{Account, AccountId, Worker};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const USDC: u128 = 1_000_000;

fn wasm(name: &str) -> Vec<u8> {
    let path = format!(
        "{}/../../target/near/{name}/{name}.wasm",
        env!("CARGO_MANIFEST_DIR")
    );

    std::fs::read(&path)
        .unwrap_or_else(|_| panic!("{path} is missing: run ./scripts/build.sh --all"))
}

struct Env {
    worker: Worker<Sandbox>,
    operator: Account,
    payer: Account,
    shop: Account,
    kite: Account,
    stranger: Account,
    factory: AccountId,
    usdc: AccountId,
    usdt: AccountId,
    wrap: AccountId,
    router: AccountId,
    implementation: String,
}

async fn account(worker: &Worker<Sandbox>, name: &str, near: u128) -> anyhow::Result<Account> {
    Ok(worker
        .root_account()?
        .create_subaccount(name)
        .initial_balance(NearToken::from_near(near))
        .transact()
        .await?
        .into_result()?)
}

async fn ok(result: ExecutionFinalResult) -> anyhow::Result<ExecutionFinalResult> {
    if result.is_failure() {
        anyhow::bail!("transaction failed: {:#?}", result.failures());
    }

    Ok(result)
}

async fn token(worker: &Worker<Sandbox>, name: &str) -> anyhow::Result<AccountId> {
    let account = account(worker, name, 20).await?;
    let contract = account.deploy(&wasm("mock_ft")).await?.into_result()?;

    ok(contract.call("new").transact().await?).await?;
    Ok(contract.id().clone())
}

async fn setup() -> anyhow::Result<Env> {
    let worker = near_workspaces::sandbox().await?;

    let publisher = account(&worker, "publisher", 60).await?;
    let publisher = publisher.deploy(&wasm("publisher")).await?.into_result()?;
    let deposit_wasm = wasm("kite");

    ok(publisher
        .call("publish")
        .args(deposit_wasm.clone())
        .max_gas()
        .transact()
        .await?)
    .await?;

    let implementation = bs58::encode(Sha256::digest(&deposit_wasm)).into_string();

    let usdc = token(&worker, "usdc").await?;
    let usdt = token(&worker, "usdt").await?;
    let wrap = token(&worker, "wrap").await?;

    let operator = account(&worker, "operator", 50).await?;
    let stranger = account(&worker, "stranger", 10).await?;
    let admin = account(&worker, "admin", 10).await?;
    let payer = account(&worker, "payer", 50).await?;
    let shop = account(&worker, "shop", 10).await?;
    let kite = account(&worker, "kite", 10).await?;

    let factory = account(&worker, "factory", 20).await?;
    let factory = factory.deploy(&wasm("factory")).await?.into_result()?;

    ok(factory
        .call("new")
        .args_json(json!({
            "admin": admin.id(),
            "operator": operator.id(),
            "implementation": implementation,
        }))
        .transact()
        .await?)
    .await?;

    let router = account(&worker, "router", 20).await?;
    let router = router.deploy(&wasm("mock_router")).await?.into_result()?;

    ok(router
        .call("new")
        .args_json(json!({ "numerator": "1", "denominator": "1" }))
        .transact()
        .await?)
    .await?;

    let root = worker.root_account()?;
    for token in [&usdc, &usdt, &wrap] {
        ok(root
            .call(token, "mint")
            .args_json(
                json!({ "account_id": router.id(), "amount": (1_000_000 * USDC).to_string() }),
            )
            .transact()
            .await?)
        .await?;
    }

    // the server registers recipients on the settlement token before settling
    for account in [&shop, &kite] {
        ok(root
            .call(&usdc, "mint")
            .args_json(json!({ "account_id": account.id(), "amount": "0" }))
            .transact()
            .await?)
        .await?;
    }

    Ok(Env {
        worker,
        operator,
        payer,
        shop,
        kite,
        stranger,
        factory: factory.id().clone(),
        usdc,
        usdt,
        wrap,
        router: router.id().clone(),
        implementation,
    })
}

impl Env {
    async fn now(&self) -> anyhow::Result<u64> {
        Ok(self.worker.view_block().await?.timestamp() / 1_000_000_000)
    }

    async fn transfer(
        &self,
        pay_in: Asset,
        settlement: Asset,
        amount_in: u128,
    ) -> anyhow::Result<Transfer> {
        let swap = (pay_in != settlement).then(|| Swap {
            router: self.router.as_str().parse().unwrap(),
            pool: SwapPool::Dcl(format!("{}|{}|100", self.usdc, self.usdt)),
            wrap: self.wrap.as_str().parse().unwrap(),
        });

        Ok(Transfer {
            id: format!("t-{}", self.now().await?),
            pay_in,
            amount_in: amount_in.into(),
            settlement_token: settlement,
            settlement_recipient: self.shop.id().as_str().parse().unwrap(),
            min_amount_out: (USDC * 99 / 100).into(),
            fees: vec![Fee {
                id: "fee-1".into(),
                recipient: self.kite.id().as_str().parse().unwrap(),
                amount: (USDC / 100).into(),
            }],
            refund_account: self.payer.id().as_str().parse().unwrap(),
            expires_at: (self.now().await? + 3600).into(),
            swap,
            implementation: self.implementation.parse().unwrap(),
        })
    }

    fn usdc_asset(&self) -> Asset {
        Asset::Token(self.usdc.as_str().parse().unwrap())
    }

    fn usdt_asset(&self) -> Asset {
        Asset::Token(self.usdt.as_str().parse().unwrap())
    }

    async fn predict(&self, transfer: &Transfer) -> anyhow::Result<AccountId> {
        let address: String = self
            .worker
            .view(&self.factory, "predict_address")
            .args_json(json!({ "transfer": transfer }))
            .await?
            .json()?;

        Ok(address.parse()?)
    }

    async fn deploy(&self, transfer: &Transfer) -> anyhow::Result<AccountId> {
        let deposit: String = self
            .worker
            .view(&self.factory, "deploy_deposit")
            .args_json(json!({ "transfer": transfer }))
            .await?
            .json()?;

        let result = ok(self
            .operator
            .call(&self.factory, "deploy")
            .args_json(json!({ "transfer": transfer }))
            .deposit(NearToken::from_yoctonear(deposit.parse()?))
            .gas(near_workspaces::types::Gas::from_tgas(100))
            .transact()
            .await?)
        .await?;

        Ok(result.json::<String>()?.parse()?)
    }

    async fn register(&self, transfer: &Transfer) -> anyhow::Result<ExecutionFinalResult> {
        let deposit: String = self
            .worker
            .view(&self.factory, "register_deposit")
            .args_json(json!({ "transfer": transfer }))
            .await?
            .json()?;

        ok(self
            .operator
            .call(&self.factory, "register")
            .args_json(json!({ "transfer": transfer }))
            .deposit(NearToken::from_yoctonear(deposit.parse()?))
            .gas(near_workspaces::types::Gas::from_tgas(50))
            .transact()
            .await?)
        .await
    }

    async fn near_of(&self, account: &AccountId) -> anyhow::Result<u128> {
        Ok(self
            .worker
            .view_account(account)
            .await?
            .balance
            .as_yoctonear())
    }

    async fn call(
        &self,
        who: &Account,
        method: &str,
        transfer: &Transfer,
        extra: Value,
    ) -> anyhow::Result<ExecutionFinalResult> {
        let mut args = json!({ "transfer": transfer });
        if let (Some(args), Some(extra)) = (args.as_object_mut(), extra.as_object()) {
            args.extend(extra.clone());
        }

        Ok(who
            .call(&self.factory, method)
            .args_json(args)
            .max_gas()
            .transact()
            .await?)
    }

    async fn settle_through_swap(
        &self,
        transfer: &Transfer,
        deposit: &AccountId,
    ) -> anyhow::Result<ExecutionFinalResult> {
        let mut result = ok(self
            .call(&self.operator, "settle", transfer, json!({}))
            .await?)
        .await?;

        for _ in 0..3 {
            if self.status(deposit).await? != "Executing" {
                break;
            }

            assert!(
                result
                    .logs()
                    .iter()
                    .any(|l| l.contains("OutputPending") || l.contains("OutOfGas")),
                "{:#?}",
                result.logs()
            );

            result = ok(self
                .call(&self.operator, "settle", transfer, json!({}))
                .await?)
            .await?;
        }

        Ok(result)
    }

    async fn status(&self, deposit: &AccountId) -> anyhow::Result<String> {
        Ok(self.worker.view(deposit, "get_status").await?.json()?)
    }

    async fn balance(&self, token: &AccountId, account: &AccountId) -> anyhow::Result<u128> {
        let balance: String = self
            .worker
            .view(token, "ft_balance_of")
            .args_json(json!({ "account_id": account }))
            .await?
            .json()?;

        Ok(balance.parse()?)
    }

    async fn pay_token(
        &self,
        token: &AccountId,
        deposit: &AccountId,
        amount: u128,
    ) -> anyhow::Result<()> {
        let root = self.worker.root_account()?;

        ok(root
            .call(token, "mint")
            .args_json(json!({ "account_id": self.payer.id(), "amount": amount.to_string() }))
            .transact()
            .await?)
        .await?;

        ok(self
            .payer
            .call(token, "ft_transfer")
            .args_json(json!({ "receiver_id": deposit, "amount": amount.to_string() }))
            .deposit(NearToken::from_yoctonear(1))
            .transact()
            .await?)
        .await?;

        Ok(())
    }
}

fn logs_with(result: &ExecutionFinalResult, event: &str) -> usize {
    result
        .logs()
        .iter()
        .filter(|l| l.contains(&format!("\"event\":\"{event}\"")))
        .count()
}

#[tokio::test]
async fn deploys_at_the_predicted_account_and_settles_a_token_payment() -> anyhow::Result<()> {
    let env = setup().await?;
    let transfer = env
        .transfer(env.usdc_asset(), env.usdc_asset(), USDC + 500)
        .await?;

    let predicted = env.predict(&transfer).await?;
    assert!(predicted.as_str().starts_with("0s"));

    let deployed = env.deploy(&transfer).await?;
    assert_eq!(deployed, predicted);
    assert_eq!(env.status(&deployed).await?, "Open");

    // overpaid by 100
    env.pay_token(&env.usdc, &deployed, USDC + 600).await?;

    let settled = ok(env
        .call(&env.operator, "settle", &transfer, json!({}))
        .await?)
    .await?;

    assert_eq!(logs_with(&settled, "settled"), 1, "{:#?}", settled.logs());
    assert_eq!(logs_with(&settled, "fee_paid"), 1);
    assert_eq!(logs_with(&settled, "excess_refunded"), 1);

    assert_eq!(env.status(&deployed).await?, "Settled");
    assert_eq!(env.balance(&env.usdc, env.kite.id()).await?, USDC / 100);
    assert_eq!(env.balance(&env.usdc, env.payer.id()).await?, 100);
    assert_eq!(
        env.balance(&env.usdc, env.shop.id()).await?,
        USDC + 500 - USDC / 100
    );

    Ok(())
}

#[tokio::test]
async fn only_the_operator_deploys_and_settles() -> anyhow::Result<()> {
    let env = setup().await?;
    let transfer = env
        .transfer(env.usdc_asset(), env.usdc_asset(), USDC)
        .await?;

    let refused = env
        .stranger
        .call(&env.factory, "deploy")
        .args_json(json!({ "transfer": &transfer }))
        .deposit(NearToken::from_near(1))
        .max_gas()
        .transact()
        .await?;

    assert!(format!("{:?}", refused.failures()).contains("OnlyOperator"));

    env.deploy(&transfer).await?;
    let refused = env
        .call(&env.stranger, "settle", &transfer, json!({}))
        .await?;

    assert!(format!("{:?}", refused.failures()).contains("OnlyOperator"));
    Ok(())
}

#[tokio::test]
async fn waits_for_the_full_payment() -> anyhow::Result<()> {
    let env = setup().await?;
    let transfer = env
        .transfer(env.usdc_asset(), env.usdc_asset(), USDC)
        .await?;

    let deployed = env.deploy(&transfer).await?;
    env.pay_token(&env.usdc, &deployed, USDC - 1).await?;

    let result = ok(env
        .call(&env.operator, "settle", &transfer, json!({}))
        .await?)
    .await?;

    assert_eq!(logs_with(&result, "settle_stopped"), 1);
    assert!(result
        .logs()
        .iter()
        .any(|l| l.contains("InsufficientPayment")));
    assert_eq!(env.status(&deployed).await?, "Open");

    Ok(())
}

#[tokio::test]
async fn swaps_another_token_through_the_router() -> anyhow::Result<()> {
    let env = setup().await?;
    let transfer = env
        .transfer(env.usdt_asset(), env.usdc_asset(), USDC)
        .await?;

    let deployed = env.deploy(&transfer).await?;
    env.pay_token(&env.usdt, &deployed, USDC).await?;

    let settled = env.settle_through_swap(&transfer, &deployed).await?;
    assert_eq!(
        env.status(&deployed).await?,
        "Settled",
        "{:#?}",
        settled.logs()
    );

    assert_eq!(
        env.balance(&env.usdc, env.shop.id()).await?,
        USDC - USDC / 100
    );

    assert_eq!(
        env.balance(&env.usdt, &env.router).await?,
        1_000_000 * USDC + USDC
    );

    Ok(())
}

#[tokio::test]
async fn a_swap_below_the_minimum_stays_open_and_can_be_refunded() -> anyhow::Result<()> {
    let env = setup().await?;
    let root = env.worker.root_account()?;

    ok(root
        .call(&env.router, "set_rate")
        .args_json(json!({ "numerator": "90", "denominator": "100" }))
        .transact()
        .await?)
    .await?;

    let transfer = env
        .transfer(env.usdt_asset(), env.usdc_asset(), USDC)
        .await?;

    let deployed = env.deploy(&transfer).await?;
    env.pay_token(&env.usdt, &deployed, USDC).await?;

    let result = ok(env
        .call(&env.operator, "settle", &transfer, json!({}))
        .await?)
    .await?;

    assert!(
        result.logs().iter().any(|l| l.contains("SwapFailed")),
        "{:#?}",
        result.logs()
    );
    assert_eq!(env.status(&deployed).await?, "Open");
    assert_eq!(env.balance(&env.usdt, &deployed).await?, USDC);

    let refunded = ok(env
        .call(&env.operator, "refund", &transfer, json!({}))
        .await?)
    .await?;

    assert_eq!(logs_with(&refunded, "refunded"), 1);
    assert_eq!(env.status(&deployed).await?, "Refunded");
    assert_eq!(env.balance(&env.usdt, env.payer.id()).await?, USDC);
    Ok(())
}

#[tokio::test]
async fn settles_native_near() -> anyhow::Result<()> {
    let env = setup().await?;
    let one = NearToken::from_near(1).as_yoctonear();

    let mut transfer = env.transfer(Asset::Native, Asset::Native, one).await?;
    transfer.min_amount_out = one.into();
    transfer.fees = vec![];

    let deployed = env.deploy(&transfer).await?;

    ok(env
        .payer
        .transfer_near(&deployed, NearToken::from_near(1))
        .await?)
    .await?;

    let shop_before = env.worker.view_account(env.shop.id()).await?.balance;
    let settled = ok(env
        .call(&env.operator, "settle", &transfer, json!({}))
        .await?)
    .await?;

    assert_eq!(
        env.status(&deployed).await?,
        "Settled",
        "{:#?}",
        settled.logs()
    );

    let shop_after = env.worker.view_account(env.shop.id()).await?.balance;
    assert_eq!(shop_after.as_yoctonear() - shop_before.as_yoctonear(), one);
    Ok(())
}

#[tokio::test]
async fn swaps_native_near_through_wrapped_near() -> anyhow::Result<()> {
    let env = setup().await?;
    let mut transfer = env.transfer(Asset::Native, env.usdc_asset(), USDC).await?;

    transfer.swap = Some(Swap {
        router: env.router.as_str().parse().unwrap(),
        pool: SwapPool::RefV1(7.into()),
        wrap: env.wrap.as_str().parse().unwrap(),
    });

    let deployed = env.deploy(&transfer).await?;
    ok(env
        .payer
        .transfer_near(&deployed, NearToken::from_yoctonear(USDC))
        .await?)
    .await?;

    let settled = env.settle_through_swap(&transfer, &deployed).await?;
    assert_eq!(
        env.status(&deployed).await?,
        "Settled",
        "{:#?}",
        settled.logs()
    );
    assert_eq!(
        env.balance(&env.usdc, env.shop.id()).await?,
        USDC - USDC / 100
    );

    Ok(())
}

#[tokio::test]
async fn anyone_refunds_after_expiry() -> anyhow::Result<()> {
    let env = setup().await?;
    let mut transfer = env
        .transfer(env.usdc_asset(), env.usdc_asset(), USDC)
        .await?;

    transfer.expires_at = (env.now().await? + 5).into();
    let deployed = env.deploy(&transfer).await?;
    env.pay_token(&env.usdc, &deployed, USDC / 2).await?;

    let early = env
        .call(&env.stranger, "refund", &transfer, json!({}))
        .await?;
    assert!(format!("{:?}", early.failures()).contains("OnlyOperator"));

    while env.now().await? <= transfer.expires_at.0 {
        env.worker.fast_forward(20).await?;
    }

    let refunded = ok(env
        .call(&env.stranger, "refund", &transfer, json!({}))
        .await?)
    .await?;

    assert_eq!(env.balance(&env.usdc, env.payer.id()).await?, USDC / 2);
    assert_eq!(
        logs_with(&refunded, "refunded"),
        1,
        "{:#?}",
        refunded.logs()
    );

    Ok(())
}

#[tokio::test]
async fn recovers_a_stray_token_but_not_its_own_while_open() -> anyhow::Result<()> {
    let env = setup().await?;
    let transfer = env
        .transfer(env.usdc_asset(), env.usdc_asset(), USDC)
        .await?;

    let deployed = env.deploy(&transfer).await?;
    env.pay_token(&env.usdc, &deployed, USDC).await?;

    let refused = env
        .call(
            &env.operator,
            "recover",
            &transfer,
            json!({ "token": env.usdc, "destination": null }),
        )
        .await?;

    assert!(format!("{:?}", refused.failures()).contains("RecoveryNotAllowed"));

    let root = env.worker.root_account()?;
    ok(root
        .call(&env.usdt, "storage_deposit")
        .args_json(json!({ "account_id": deployed }))
        .deposit(NearToken::from_millinear(10))
        .transact()
        .await?)
    .await?;

    env.pay_token(&env.usdt, &deployed, 42).await?;
    ok(root
        .call(&env.usdt, "mint")
        .args_json(json!({ "account_id": env.kite.id(), "amount": "0" }))
        .transact()
        .await?)
    .await?;

    let recovered = ok(env
        .call(
            &env.operator,
            "recover",
            &transfer,
            json!({ "token": env.usdt, "destination": env.kite.id() }),
        )
        .await?)
    .await?;

    assert_eq!(
        logs_with(&recovered, "recovered"),
        1,
        "{:#?}",
        recovered.logs()
    );

    assert_eq!(env.balance(&env.usdt, env.kite.id()).await?, 42);

    Ok(())
}

#[tokio::test]
async fn a_large_state_keeps_its_storage_stake_apart_from_the_payment() -> anyhow::Result<()> {
    let env = setup().await?;
    let one = NearToken::from_near(1).as_yoctonear();
    let mut transfer = env.transfer(Asset::Native, Asset::Native, one).await?;

    transfer.min_amount_out = one.into();
    transfer.fees = (0..10)
        .map(|i| Fee {
            id: format!("{i:0>36}"),
            recipient: env.kite.id().as_str().parse().unwrap(),
            amount: 1.into(),
        })
        .collect();

    let deployed = env.deploy(&transfer).await?;
    let account = env.worker.view_account(&deployed).await?;

    assert!(account.storage_usage > 770);
    assert_eq!(
        account.balance.as_yoctonear(),
        account.storage_usage as u128 * 10u128.pow(19) + DEPOSIT_RESERVE.as_yoctonear()
    );

    env.payer
        .transfer_near(&deployed, NearToken::from_yoctonear(one - 1))
        .await?;

    let short = ok(env
        .call(&env.operator, "settle", &transfer, json!({}))
        .await?)
    .await?;

    assert!(
        short
            .logs()
            .iter()
            .any(|l| l.contains("InsufficientPayment")),
        "{:#?}",
        short.logs()
    );

    env.payer
        .transfer_near(&deployed, NearToken::from_yoctonear(1))
        .await?;

    let settled = ok(env
        .call(&env.operator, "settle", &transfer, json!({}))
        .await?)
    .await?;

    assert_eq!(
        env.status(&deployed).await?,
        "Settled",
        "{:#?}",
        settled.logs()
    );

    Ok(())
}

const MILLI: u128 = 1_000_000_000_000_000_000_000;

#[tokio::test]
async fn a_registered_transfer_has_no_account_until_it_is_deployed() -> anyhow::Result<()> {
    let env = setup().await?;
    let transfer = env
        .transfer(env.usdc_asset(), env.usdc_asset(), USDC)
        .await?;

    let predicted = env.predict(&transfer).await?;

    env.register(&transfer).await?;
    assert!(env.worker.view_account(&predicted).await.is_err());

    env.pay_token(&env.usdc, &predicted, USDC).await?;
    assert_eq!(env.balance(&env.usdc, &predicted).await?, USDC);
    assert!(env.worker.view_account(&predicted).await.is_err());

    let deployed = env.deploy(&transfer).await?;
    assert_eq!(deployed, predicted);

    let settled = ok(env
        .call(&env.operator, "settle", &transfer, json!({}))
        .await?)
    .await?;
    assert_eq!(
        env.status(&deployed).await?,
        "Settled",
        "{:#?}",
        settled.logs()
    );
    assert_eq!(
        env.balance(&env.usdc, env.shop.id()).await?,
        USDC * 99 / 100
    );
    Ok(())
}

#[tokio::test]
async fn close_returns_the_reserve_and_registrations() -> anyhow::Result<()> {
    let env = setup().await?;
    let transfer = env
        .transfer(env.usdc_asset(), env.usdc_asset(), USDC)
        .await?;

    let deployed = env.deploy(&transfer).await?;
    env.pay_token(&env.usdc, &deployed, USDC).await?;

    let early = env
        .call(&env.operator, "close", &transfer, json!({}))
        .await?;
    assert!(early.is_failure());

    ok(env
        .call(&env.operator, "settle", &transfer, json!({}))
        .await?)
    .await?;

    let closed = ok(env
        .call(&env.operator, "close", &transfer, json!({}))
        .await?)
    .await?;

    assert_eq!(logs_with(&closed, "closed"), 1, "{:#?}", closed.logs());

    let returned: String = closed.json::<String>()?;
    let returned: u128 = returned.parse()?;

    assert!(returned > MILLI + MILLI, "{returned}");

    // left with dust, and unregistered
    assert!(env.near_of(&deployed).await? <= MILLI / 1000);
    let registered: Value = env
        .worker
        .view(&env.usdc, "storage_balance_of")
        .args_json(json!({ "account_id": deployed }))
        .await?
        .json()?;
    assert!(registered.is_null(), "{registered}");

    // only the operator closes
    let stranger = env
        .call(&env.stranger, "close", &transfer, json!({}))
        .await?;

    assert!(stranger.is_failure());

    Ok(())
}

#[tokio::test]
async fn native_near_paid_before_the_account_exists() -> anyhow::Result<()> {
    let env = setup().await?;
    let one = NearToken::from_near(1).as_yoctonear();

    let mut transfer = env.transfer(Asset::Native, Asset::Native, one).await?;
    transfer.min_amount_out = one.into();
    transfer.fees = vec![];

    let predicted = env.predict(&transfer).await?;

    ok(env
        .payer
        .transfer_near(&predicted, NearToken::from_near(1))
        .await?)
    .await?;

    assert_eq!(env.near_of(&predicted).await?, one);

    env.deploy(&transfer).await?;
    env.deploy(&transfer).await?; // deploying again changes nothing: fund sends the second reserve back

    assert_eq!(env.status(&predicted).await?, "Open");

    let shop_before = env.near_of(env.shop.id()).await?;
    let payer_before = env.near_of(env.payer.id()).await?;
    let settled = ok(env
        .call(&env.operator, "settle", &transfer, json!({}))
        .await?)
    .await?;

    assert_eq!(
        env.status(&predicted).await?,
        "Settled",
        "{:#?}",
        settled.logs()
    );
    assert_eq!(env.near_of(env.shop.id()).await? - shop_before, one);
    assert_eq!(
        logs_with(&settled, "excess_refunded"),
        0,
        "{:#?}",
        settled.logs()
    );
    assert_eq!(env.near_of(env.payer.id()).await?, payer_before);

    let closed = ok(env
        .call(&env.operator, "close", &transfer, json!({}))
        .await?)
    .await?;
    let returned: u128 = closed.json::<String>()?.parse()?;
    assert!(returned > MILLI * 9 / 10, "{returned}");
    Ok(())
}

#[tokio::test]
async fn close_after_a_refund() -> anyhow::Result<()> {
    let env = setup().await?;
    let transfer = env
        .transfer(env.usdc_asset(), env.usdc_asset(), USDC)
        .await?;

    let deployed = env.deploy(&transfer).await?;
    env.pay_token(&env.usdc, &deployed, USDC).await?;
    ok(env
        .call(&env.operator, "refund", &transfer, json!({}))
        .await?)
    .await?;
    assert_eq!(env.status(&deployed).await?, "Refunded");

    let closed = ok(env
        .call(&env.operator, "close", &transfer, json!({}))
        .await?)
    .await?;
    let returned: u128 = closed.json::<String>()?.parse()?;
    assert!(returned > MILLI + MILLI, "{returned}");
    Ok(())
}
