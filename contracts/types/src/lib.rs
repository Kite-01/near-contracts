use near_sdk::json_types::{Base58CryptoHash, U128, U64};
use near_sdk::state_init::{StateInit, StateInitV1};
use near_sdk::{near, AccountId, CryptoHash, GlobalContractId, NearToken};

pub const MAX_FEES: usize = 10;
pub const MAX_ID_LEN: usize = 64;

pub const DEPOSIT_RESERVE: NearToken = NearToken::from_millinear(1);
pub const STATE_INIT_DEPOSIT: NearToken = NearToken::from_millinear(30);
pub const ZERO_BALANCE_STORAGE_LIMIT: u64 = 770;
pub const ACCOUNT_OVERHEAD: u64 = 200;
pub const STORAGE_MARGIN: u64 = 512;

#[near(serializers = [borsh, json])]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Asset {
    Native,
    Token(AccountId),
}

impl Asset {
    pub fn token(&self) -> Option<&AccountId> {
        match self {
            Asset::Native => None,
            Asset::Token(id) => Some(id),
        }
    }
}

#[near(serializers = [borsh, json])]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SwapPool {
    /// Rhea DCL pool id, `token_a|token_b|fee`
    Dcl(String),
    /// Ref v1 pool index
    RefV1(U64),
}

#[near(serializers = [borsh, json])]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Swap {
    pub router: AccountId,
    pub pool: SwapPool,
    pub wrap: AccountId,
}

#[near(serializers = [borsh, json])]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fee {
    pub id: String,
    pub recipient: AccountId,
    pub amount: U128,
}

#[near(serializers = [borsh, json])]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transfer {
    pub id: String,
    pub pay_in: Asset,
    pub amount_in: U128,
    pub settlement_token: Asset,
    pub settlement_recipient: AccountId,
    pub min_amount_out: U128,
    pub fees: Vec<Fee>,
    pub refund_account: AccountId,
    pub expires_at: U64, // unix seconds.
    pub swap: Option<Swap>,
    pub implementation: Base58CryptoHash,
}

impl Transfer {
    pub fn total_fees(&self) -> u128 {
        self.fees.iter().map(|f| f.amount.0).sum()
    }

    pub fn needs_swap(&self) -> bool {
        self.pay_in != self.settlement_token
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if self.id.is_empty() || self.id.len() > MAX_ID_LEN {
            return Err("InvalidConfig: id");
        }

        if self.fees.len() > MAX_FEES {
            return Err("TooManyFees");
        }

        for fee in &self.fees {
            if fee.amount.0 == 0 || fee.id.is_empty() || fee.id.len() > MAX_ID_LEN {
                return Err("InvalidConfig: fee");
            }
        }

        if self.amount_in.0 == 0 {
            return Err("InvalidConfig: amount_in");
        }

        let total = self
            .fees
            .iter()
            .try_fold(0u128, |acc, f| acc.checked_add(f.amount.0))
            .ok_or("InvalidConfig: fees overflow")?;

        if self.min_amount_out.0 < total {
            return Err("InvalidConfig: fees exceed min_amount_out");
        }

        if self.needs_swap() {
            let Some(swap) = &self.swap else {
                return Err("ZeroAddress: swap");
            };

            // native and wrapped NEAR are the same token to the router.
            let wrap = Asset::Token(swap.wrap.clone());
            let pair = (&self.pay_in, &self.settlement_token);
            if pair == (&Asset::Native, &wrap) || pair == (&wrap, &Asset::Native) {
                return Err("InvalidConfig: native and wrapped NEAR");
            }
        }

        Ok(())
    }

    pub fn tokens_to_register(&self) -> Vec<AccountId> {
        let mut tokens: Vec<AccountId> = Vec::new();
        let mut push = |id: &AccountId| {
            if !tokens.contains(id) {
                tokens.push(id.clone());
            }
        };

        if let Some(t) = self.pay_in.token() {
            push(t);
        }

        if let Some(t) = self.settlement_token.token() {
            push(t);
        }

        if self.needs_swap() {
            if let Some(swap) = &self.swap {
                if self.pay_in == Asset::Native || self.settlement_token == Asset::Native {
                    push(&swap.wrap);
                }
            }
        }

        tokens
    }
}

#[near(serializers = [borsh, json])]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Uninitialized,
    Open,
    Executing,
    Settled,
    Refunded,
}

#[near(serializers = [borsh, json])]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Step {
    #[default]
    CheckBalance,
    ReadOutBefore,
    Wrap,
    Swap,
    ReadOutAfter,
    Unwrap,
    Pay,
    Done,
}

#[near(serializers = [borsh, json])]
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Progress {
    pub step: Step,
    pub excess: U128,
    pub out_before: U128,
    pub amount_out: U128,
    pub wrapped: bool,
    pub paid: u16,
    pub lock_until: U64,
}

pub const RECIPIENT_BIT: u16 = 1 << 15;

#[near(serializers = [borsh, json])]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DepositState {
    pub factory: AccountId,
    pub transfer: Transfer,
    pub status: Status,
    pub progress: Progress,
}

impl DepositState {
    pub fn initial(factory: AccountId, transfer: Transfer) -> Self {
        Self {
            factory,
            transfer,
            status: Status::Uninitialized,
            progress: Progress::default(),
        }
    }

    pub fn state_init(&self) -> StateInit {
        let code: CryptoHash = self.transfer.implementation.into();
        let value = near_sdk::borsh::to_vec(self).expect("borsh");
        StateInit::V1(
            StateInitV1::code(GlobalContractId::CodeHash(code))
                .with_data_entry(b"STATE".to_vec(), value),
        )
    }

    pub fn state_init_deposit(&self) -> NearToken {
        let bytes = near_sdk::borsh::to_vec(self).unwrap_or_default().len() as u64;
        if bytes + ACCOUNT_OVERHEAD <= ZERO_BALANCE_STORAGE_LIMIT {
            NearToken::from_yoctonear(0)
        } else {
            STATE_INIT_DEPOSIT
        }
    }

    pub fn account_id(&self) -> AccountId {
        self.state_init().derive_account_id()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn sample() -> Transfer {
        Transfer {
            id: "0b8f6c1e-6d1f-4c7e-9f00-5a1b2c3d4e5f".into(),
            pay_in: Asset::Token("usdc.near".parse().unwrap()),
            amount_in: U128(1_000_000),
            settlement_token: Asset::Token("usdc.near".parse().unwrap()),
            settlement_recipient: "shop.near".parse().unwrap(),
            min_amount_out: U128(1_000_000),
            fees: vec![Fee {
                id: "fee-1".into(),
                recipient: "kite.near".parse().unwrap(),
                amount: U128(10_000),
            }],
            refund_account: "payer.near".parse().unwrap(),
            expires_at: U64(1_800_000_000),
            swap: None,
            implementation: Base58CryptoHash::from([7u8; 32]),
        }
    }

    #[test]
    fn validates_like_evm() {
        let mut t = sample();
        assert!(t.validate().is_ok());
        t.min_amount_out = U128(1);
        assert_eq!(
            t.validate(),
            Err("InvalidConfig: fees exceed min_amount_out")
        );

        let mut t = sample();
        t.settlement_token = Asset::Native;
        assert_eq!(t.validate(), Err("ZeroAddress: swap"));

        let mut t = sample();
        t.fees = (0..11)
            .map(|i| Fee {
                id: i.to_string(),
                recipient: "a.near".parse().unwrap(),
                amount: U128(1),
            })
            .collect();
        assert_eq!(t.validate(), Err("TooManyFees"));
    }

    #[test]
    fn derives_a_0s_account() {
        let state = DepositState::initial("factory.near".parse().unwrap(), sample());
        let id = state.account_id();
        assert!(id.as_str().starts_with("0s"));
        assert_eq!(id.as_str().len(), 42);

        let mut other = state.clone();
        other.transfer.amount_in = U128(2);
        assert_ne!(id, other.account_id());
    }

    #[test]
    fn registers_wrap_for_native_swaps() {
        let mut t = sample();
        t.pay_in = Asset::Native;
        t.swap = Some(Swap {
            router: "v2.ref-finance.near".parse().unwrap(),
            pool: SwapPool::RefV1(U64(3)),
            wrap: "wrap.near".parse().unwrap(),
        });

        let tokens: Vec<String> = t
            .tokens_to_register()
            .iter()
            .map(|a| a.to_string())
            .collect();
        assert_eq!(tokens, vec!["usdc.near", "wrap.near"]);
    }

    #[test]
    fn a_typical_state_needs_no_state_init_deposit() {
        let state = DepositState::initial("kite-factory.near".parse().unwrap(), sample());
        assert_eq!(state.state_init_deposit().as_yoctonear(), 0);
    }

    #[test]
    fn initial_state_fits_state_init_deposit() {
        let mut t = sample();
        t.id = "x".repeat(MAX_ID_LEN);
        t.fees = (0..MAX_FEES)
            .map(|i| Fee {
                id: format!("{:0>64}", i),
                recipient: "a".repeat(64).parse().unwrap(),
                amount: U128(1),
            })
            .collect();

        let state = DepositState::initial("f".repeat(64).parse().unwrap(), t);
        let bytes = near_sdk::borsh::to_vec(&state).unwrap().len() as u64;
        let needed = (bytes + ACCOUNT_OVERHEAD + STORAGE_MARGIN) as u128 * 10u128.pow(19);
        assert_eq!(state.state_init_deposit(), STATE_INIT_DEPOSIT);
        assert!(
            needed < STATE_INIT_DEPOSIT.as_yoctonear(),
            "{bytes} bytes need {needed}"
        );
    }
}

#[cfg(test)]
mod vector {
    use super::*;

    #[test]
    fn fixed_vector() {
        let transfer = Transfer {
            id: "7d0c3a52-2f1e-4b8a-9c61-0f3e2d1c4b5a".into(),
            pay_in: Asset::Native,
            amount_in: U128(1_500_000_000_000_000_000_000_000),
            settlement_token: Asset::Token("usdc.near".parse().unwrap()),
            settlement_recipient: "shop.near".parse().unwrap(),
            min_amount_out: U128(4_950_000),
            fees: vec![Fee {
                id: "1f2e3d4c-5b6a-4978-8695-a4b3c2d1e0f9".into(),
                recipient: "kite.near".parse().unwrap(),
                amount: U128(50_000),
            }],
            refund_account: "payer.near".parse().unwrap(),
            expires_at: U64(1_800_000_000),
            swap: Some(Swap {
                router: "dclv2.ref-labs.near".parse().unwrap(),
                pool: SwapPool::Dcl("usdc.near|wrap.near|100".into()),
                wrap: "wrap.near".parse().unwrap(),
            }),
            implementation: "4reLvkAWfqk5fsqio1KLudk46cqRz9erQdaHkWZKMJDZ"
                .parse()
                .unwrap(),
        };

        let state = DepositState::initial("kite-factory.near".parse().unwrap(), transfer);
        let borsh = near_sdk::borsh::to_vec(&state).unwrap();
        println!("STATE {}", hex(&borsh));
        println!("ACCOUNT {}", state.account_id());
        assert_eq!(state.account_id().as_str(), EXPECTED);
    }

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    const EXPECTED: &str = "0s2b5f59b451afc7c774c8836ceb8266bf80487c70";
}
