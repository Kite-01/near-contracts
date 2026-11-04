use near_sdk::json_types::U128;
use near_sdk::serde_json::{self, Value};
use near_sdk::{env, near, AccountId, Gas, NearToken, PanicOnDefault, Promise, PromiseOrValue};

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct MockRouter {
    numerator: u128,
    denominator: u128,
}

#[near]
impl MockRouter {
    #[init]
    pub fn new(numerator: U128, denominator: U128) -> Self {
        Self {
            numerator: numerator.0,
            denominator: denominator.0,
        }
    }

    pub fn set_rate(&mut self, numerator: U128, denominator: U128) {
        self.numerator = numerator.0;
        self.denominator = denominator.0;
    }

    pub fn ft_on_transfer(
        &mut self,
        sender_id: AccountId,
        amount: U128,
        msg: String,
    ) -> PromiseOrValue<U128> {
        let msg: Value = serde_json::from_str(&msg).unwrap_or_else(|_| env::panic_str("bad msg"));
        let (token_out, min) = match msg.get("Swap") {
            Some(swap) => (&swap["output_token"], &swap["min_output_amount"]),
            None => (
                &msg["actions"][0]["token_out"],
                &msg["actions"][0]["min_amount_out"],
            ),
        };

        let token_out: AccountId = token_out.as_str().expect("token_out").parse().unwrap();
        let min: u128 = min.as_str().expect("min").parse().unwrap();

        let out = amount.0 * self.numerator / self.denominator;
        if out < min {
            return PromiseOrValue::Value(amount);
        }

        Promise::new(token_out)
            .function_call(
                "ft_transfer",
                serde_json::json!({ "receiver_id": sender_id, "amount": U128(out) }).to_string(),
                NearToken::from_yoctonear(1),
                Gas::from_tgas(10),
            )
            .detach();
        PromiseOrValue::Value(U128(0))
    }
}
