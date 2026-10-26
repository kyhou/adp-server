//! Pure domain rules for experimental ADP split settlement version 1.
use nostr::{Event, EventBuilder, Kind};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

pub const TERMS_KIND: u16 = 1032;
pub const QUOTE_KIND: u16 = 1033;
pub const RECOVERY_KIND: u16 = 1034;
pub const SETTLEMENT_VERSION: u16 = 1;
pub const PAYMENT_WINDOW_SECS: u64 = 1_800;
pub const CONFIRMATION_WINDOW_SECS: u64 = 86_400;
pub const RECOVERY_CLAIM_GRACE_SECS: u64 = 3_600;
pub const REFUND_DUE_SECS: u64 = 604_800;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FeePolicy {
    None,
    Fixed {
        amount_msat: u64,
    },
    BasisPoints {
        basis_points: u64,
        #[serde(default)]
        minimum_msat: Option<u64>,
        #[serde(default)]
        maximum_msat: Option<u64>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommercialTerms {
    pub version: u16,
    pub operator_pubkey: String,
    pub payment_destination: String,
    pub fee: FeePolicy,
    #[serde(default)]
    pub limitations: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptedTermsRef {
    pub operator_pubkey: String,
    pub terms_event_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerOperator {
    pub url: String,
    pub operator_pubkey: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaymentRole {
    Developer,
    Operator,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaymentLeg {
    pub leg_id: String,
    pub role: PaymentRole,
    pub recipient_pubkey: String,
    pub destination: String,
    pub amount_msat: u64,
    #[serde(default)]
    pub accepted_terms_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefundDestination {
    pub endpoint: String,
    pub min_sendable_msat: u64,
    pub max_sendable_msat: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoicePreflight {
    pub leg_id: String,
    pub recipient_pubkey: String,
    pub amount_msat: u64,
    pub payment_hash: String,
    pub lnurl_origin: String,
    pub callback: String,
    pub lsp_pubkey: String,
    pub invoice_expires_at: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuoteState {
    Draft,
    Preflighting,
    Active,
    Failed,
    Settled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedQuote {
    pub version: u16,
    pub quote_id: String,
    pub buyer_pubkey: String,
    pub listing_event_id: String,
    pub listing_coordinate: String,
    pub processor_pubkey: String,
    pub gross_msat: u64,
    pub issued_at: u64,
    pub payment_expires_at: u64,
    pub confirmation_expires_at: u64,
    pub refund_destination_commitment: String,
    pub legs: Vec<PaymentLeg>,
    pub state: QuoteState,
    #[serde(default)]
    pub failure_at: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AggregateProofLeg {
    pub leg_id: String,
    pub role: PaymentRole,
    pub recipient_pubkey: String,
    pub amount_msat: u64,
    pub payment_hash: String,
    pub proof_type: String,
    pub proof_reference: String,
    #[serde(default)]
    pub accepted_terms_id: Option<String>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SettlementError {
    #[error("unsupported settlement version")]
    Version,
    #[error("fixed fee must be positive")]
    FixedFeeZero,
    #[error("basis points must be positive")]
    BasisPointsZero,
    #[error("fee bounds are invalid")]
    InvalidBounds,
    #[error("fee arithmetic overflow")]
    Overflow,
    #[error("operator terms coverage is missing or conflicting: {0}")]
    TermsCoverage(String),
    #[error("aggregate operator fees must be less than gross")]
    FeesConsumeGross,
    #[error("split settlement only supports SAT, SATS, or exactly convertible BTC")]
    UnsupportedCurrency,
    #[error("invalid amount")]
    InvalidAmount,
    #[error("quote invariant failed: {0}")]
    Quote(String),
    #[error("invoice preflight mismatch: {0}")]
    Preflight(String),
    #[error("refund destination cannot receive every possible principal refund")]
    RefundDestination,
    #[error("aggregate proof invariant failed: {0}")]
    Aggregate(String),
    #[error("serialization failed")]
    Serialization,
    #[error("invalid signed settlement event: {0}")]
    Event(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum TermsPayload {
    Offer {
        terms: CommercialTerms,
    },
    Withdraw {
        version: u16,
        terms_event_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryEvidence {
    pub version: u16,
    pub quote_id: String,
    pub quote_event_id: String,
    pub buyer_pubkey: String,
    pub processor_pubkey: String,
    pub failure_at: u64,
    pub paid_leg_ids: Vec<String>,
    pub evidence_commitment: String,
}

pub fn build_terms_event_builder(payload: &TermsPayload) -> Result<EventBuilder, SettlementError> {
    match payload {
        TermsPayload::Offer { terms } if terms.version != SETTLEMENT_VERSION => {
            return Err(SettlementError::Version)
        }
        TermsPayload::Withdraw { version, .. } if *version != SETTLEMENT_VERSION => {
            return Err(SettlementError::Version)
        }
        _ => {}
    }
    let content = serde_json::to_string(payload).map_err(|_| SettlementError::Serialization)?;
    Ok(EventBuilder::new(Kind::Custom(TERMS_KIND), content))
}

pub fn parse_terms_event(event: &Event) -> Result<TermsPayload, SettlementError> {
    if event.kind.as_u16() != TERMS_KIND || event.verify().is_err() {
        return Err(SettlementError::Event("wrong kind or signature".into()));
    }
    let payload: TermsPayload = serde_json::from_str(&event.content)
        .map_err(|_| SettlementError::Event("malformed terms payload".into()))?;
    match &payload {
        TermsPayload::Offer { terms } => {
            if terms.version != SETTLEMENT_VERSION || terms.operator_pubkey != event.pubkey.to_hex()
            {
                return Err(SettlementError::Event(
                    "offer signer is not the operator".into(),
                ));
            }
            terms.fee.amount_msat(1)?;
        }
        TermsPayload::Withdraw {
            version,
            terms_event_id,
        } => {
            if *version != SETTLEMENT_VERSION
                || terms_event_id.len() != 64
                || hex::decode(terms_event_id).is_err()
            {
                return Err(SettlementError::Event("invalid withdrawal root".into()));
            }
        }
    }
    Ok(payload)
}

pub fn verify_terms_withdrawal(withdrawal: &Event, root: &Event) -> Result<(), SettlementError> {
    let offer = parse_terms_event(root)?;
    let withdrawn = parse_terms_event(withdrawal)?;
    if !matches!(offer, TermsPayload::Offer { .. })
        || withdrawal.pubkey != root.pubkey
        || !matches!(withdrawn, TermsPayload::Withdraw { ref terms_event_id, .. } if terms_event_id == &root.id.to_hex())
    {
        return Err(SettlementError::Event(
            "withdrawal does not bind the operator root".into(),
        ));
    }
    Ok(())
}

pub fn verify_quote_event(
    event: &Event,
    decrypted_quote_json: &str,
) -> Result<SignedQuote, SettlementError> {
    if event.kind.as_u16() != QUOTE_KIND || event.verify().is_err() {
        return Err(SettlementError::Event(
            "wrong quote kind or signature".into(),
        ));
    }
    let quote: SignedQuote = serde_json::from_str(decrypted_quote_json)
        .map_err(|_| SettlementError::Event("malformed quote payload".into()))?;
    if quote.version != SETTLEMENT_VERSION || quote.processor_pubkey != event.pubkey.to_hex() {
        return Err(SettlementError::Event(
            "quote signer is not the processor".into(),
        ));
    }
    validate_leg_set(quote.gross_msat, &quote.legs)?;
    if quote.payment_expires_at
        != quote
            .issued_at
            .checked_add(PAYMENT_WINDOW_SECS)
            .ok_or(SettlementError::Overflow)?
        || quote.confirmation_expires_at
            != quote
                .issued_at
                .checked_add(CONFIRMATION_WINDOW_SECS)
                .ok_or(SettlementError::Overflow)?
    {
        return Err(SettlementError::Quote("non-canonical deadlines".into()));
    }
    Ok(quote)
}

pub fn verify_recovery_event(
    event: &Event,
    decrypted_evidence_json: &str,
) -> Result<RecoveryEvidence, SettlementError> {
    if event.kind.as_u16() != RECOVERY_KIND || event.verify().is_err() {
        return Err(SettlementError::Event(
            "wrong recovery kind or signature".into(),
        ));
    }
    let evidence: RecoveryEvidence = serde_json::from_str(decrypted_evidence_json)
        .map_err(|_| SettlementError::Event("malformed recovery payload".into()))?;
    if evidence.version != SETTLEMENT_VERSION
        || (event.pubkey.to_hex() != evidence.processor_pubkey
            && event.pubkey.to_hex() != evidence.buyer_pubkey)
    {
        return Err(SettlementError::Event(
            "recovery signer has no purchase role".into(),
        ));
    }
    Ok(evidence)
}

impl FeePolicy {
    pub fn amount_msat(&self, gross_msat: u64) -> Result<u64, SettlementError> {
        match self {
            Self::None => Ok(0),
            Self::Fixed { amount_msat: 0 } => Err(SettlementError::FixedFeeZero),
            Self::Fixed { amount_msat } => Ok(*amount_msat),
            Self::BasisPoints {
                basis_points: 0, ..
            } => Err(SettlementError::BasisPointsZero),
            Self::BasisPoints {
                basis_points,
                minimum_msat,
                maximum_msat,
            } => {
                if minimum_msat
                    .zip(*maximum_msat)
                    .is_some_and(|(min, max)| min > max)
                {
                    return Err(SettlementError::InvalidBounds);
                }
                let raw = (gross_msat as u128)
                    .checked_mul(*basis_points as u128)
                    .ok_or(SettlementError::Overflow)?
                    / 10_000;
                let mut amount = u64::try_from(raw).map_err(|_| SettlementError::Overflow)?;
                if let Some(min) = minimum_msat {
                    amount = amount.max(*min);
                }
                if let Some(max) = maximum_msat {
                    amount = amount.min(*max);
                }
                Ok(amount)
            }
        }
    }
}

pub fn validate_terms_coverage(
    servers: &[ServerOperator],
    accepted: &[AcceptedTermsRef],
) -> Result<Vec<AcceptedTermsRef>, SettlementError> {
    let operators: BTreeSet<_> = servers.iter().map(|s| s.operator_pubkey.as_str()).collect();
    let mut by_operator = BTreeMap::new();
    for reference in accepted {
        if !operators.contains(reference.operator_pubkey.as_str())
            || by_operator
                .insert(reference.operator_pubkey.clone(), reference.clone())
                .is_some()
        {
            return Err(SettlementError::TermsCoverage(
                reference.operator_pubkey.clone(),
            ));
        }
    }
    if by_operator.len() != operators.len() {
        let missing = operators
            .into_iter()
            .find(|op| !by_operator.contains_key(*op))
            .unwrap_or("unknown");
        return Err(SettlementError::TermsCoverage(missing.to_string()));
    }
    Ok(by_operator.into_values().collect())
}

pub fn build_payment_legs(
    gross_msat: u64,
    developer_pubkey: &str,
    developer_destination: &str,
    operators: &[(CommercialTerms, String)],
) -> Result<Vec<PaymentLeg>, SettlementError> {
    let mut legs = Vec::new();
    let mut fees = 0_u64;
    for (terms, terms_id) in operators {
        if terms.version != SETTLEMENT_VERSION {
            return Err(SettlementError::Version);
        }
        let amount = terms.fee.amount_msat(gross_msat)?;
        fees = fees.checked_add(amount).ok_or(SettlementError::Overflow)?;
        if amount > 0 {
            legs.push(PaymentLeg {
                leg_id: format!("operator:{}", terms.operator_pubkey),
                role: PaymentRole::Operator,
                recipient_pubkey: terms.operator_pubkey.clone(),
                destination: terms.payment_destination.clone(),
                amount_msat: amount,
                accepted_terms_id: Some(terms_id.clone()),
            });
        }
    }
    if fees >= gross_msat {
        return Err(SettlementError::FeesConsumeGross);
    }
    legs.push(PaymentLeg {
        leg_id: "developer".into(),
        role: PaymentRole::Developer,
        recipient_pubkey: developer_pubkey.into(),
        destination: developer_destination.into(),
        amount_msat: gross_msat - fees,
        accepted_terms_id: None,
    });
    legs.sort_by(|a, b| {
        (&a.amount_msat, &a.role, &a.recipient_pubkey).cmp(&(
            &b.amount_msat,
            &b.role,
            &b.recipient_pubkey,
        ))
    });
    Ok(legs)
}

pub fn parse_gross_msat(amount: &str, currency: &str) -> Result<u64, SettlementError> {
    match currency.to_ascii_uppercase().as_str() {
        "SAT" | "SATS" => amount
            .parse::<u64>()
            .ok()
            .and_then(|v| v.checked_mul(1_000))
            .ok_or(SettlementError::InvalidAmount),
        "BTC" => {
            let (whole, fraction) = amount.split_once('.').unwrap_or((amount, ""));
            if fraction.len() > 11
                || whole.starts_with('-')
                || !fraction.bytes().all(|b| b.is_ascii_digit())
            {
                return Err(SettlementError::InvalidAmount);
            }
            let whole = whole
                .parse::<u64>()
                .map_err(|_| SettlementError::InvalidAmount)?;
            let mut fractional = fraction.to_string();
            fractional.extend(std::iter::repeat_n('0', 11 - fractional.len()));
            let fraction = fractional
                .parse::<u64>()
                .map_err(|_| SettlementError::InvalidAmount)?;
            whole
                .checked_mul(100_000_000_000)
                .and_then(|v| v.checked_add(fraction))
                .ok_or(SettlementError::Overflow)
        }
        _ => Err(SettlementError::UnsupportedCurrency),
    }
}

pub fn destination_commitment(destination: &RefundDestination) -> Result<String, SettlementError> {
    canonical_commitment(destination)
}

impl SignedQuote {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        quote_id: String,
        buyer_pubkey: String,
        listing_event_id: String,
        listing_coordinate: String,
        processor_pubkey: String,
        gross_msat: u64,
        issued_at: u64,
        destination: &RefundDestination,
        legs: Vec<PaymentLeg>,
    ) -> Result<Self, SettlementError> {
        validate_leg_set(gross_msat, &legs)?;
        validate_refund_destination(destination, &legs)?;
        Ok(Self {
            version: SETTLEMENT_VERSION,
            quote_id,
            buyer_pubkey,
            listing_event_id,
            listing_coordinate,
            processor_pubkey,
            gross_msat,
            issued_at,
            payment_expires_at: issued_at
                .checked_add(PAYMENT_WINDOW_SECS)
                .ok_or(SettlementError::Overflow)?,
            confirmation_expires_at: issued_at
                .checked_add(CONFIRMATION_WINDOW_SECS)
                .ok_or(SettlementError::Overflow)?,
            refund_destination_commitment: destination_commitment(destination)?,
            legs,
            state: QuoteState::Draft,
            failure_at: None,
        })
    }

    pub fn activate(
        &mut self,
        now: u64,
        attempts: &[InvoicePreflight],
    ) -> Result<(), SettlementError> {
        if !matches!(self.state, QuoteState::Draft | QuoteState::Preflighting)
            || now > self.payment_expires_at
        {
            return Err(SettlementError::Quote("quote is not activatable".into()));
        }
        let by_leg: BTreeMap<_, _> = attempts.iter().map(|a| (a.leg_id.as_str(), a)).collect();
        if by_leg.len() != self.legs.len() {
            return Err(SettlementError::Preflight(
                "every positive leg requires exactly one invoice".into(),
            ));
        }
        for leg in &self.legs {
            let attempt = by_leg
                .get(leg.leg_id.as_str())
                .ok_or_else(|| SettlementError::Preflight(leg.leg_id.clone()))?;
            if attempt.amount_msat != leg.amount_msat
                || attempt.recipient_pubkey != leg.recipient_pubkey
                || attempt.invoice_expires_at > self.confirmation_expires_at
                || attempt.payment_hash.len() != 64
            {
                return Err(SettlementError::Preflight(leg.leg_id.clone()));
            }
        }
        self.state = QuoteState::Active;
        Ok(())
    }

    pub fn fail(&mut self, failure_at: u64) -> Result<(), SettlementError> {
        if matches!(self.state, QuoteState::Settled)
            || self
                .failure_at
                .is_some_and(|existing| existing != failure_at)
        {
            return Err(SettlementError::Quote("failure time is immutable".into()));
        }
        self.failure_at = Some(failure_at);
        self.state = QuoteState::Failed;
        Ok(())
    }

    pub fn refund_due_at(&self) -> Result<u64, SettlementError> {
        self.failure_at
            .ok_or_else(|| SettlementError::Quote("quote has not failed".into()))?
            .checked_add(REFUND_DUE_SECS)
            .ok_or(SettlementError::Overflow)
    }
}

pub fn validate_refund_destination(
    destination: &RefundDestination,
    legs: &[PaymentLeg],
) -> Result<(), SettlementError> {
    if destination.endpoint.trim().is_empty()
        || legs.iter().any(|leg| {
            leg.amount_msat < destination.min_sendable_msat
                || leg.amount_msat > destination.max_sendable_msat
        })
    {
        return Err(SettlementError::RefundDestination);
    }
    Ok(())
}

pub fn validate_leg_set(gross_msat: u64, legs: &[PaymentLeg]) -> Result<(), SettlementError> {
    let mut ids = BTreeSet::new();
    let mut sum = 0_u64;
    let mut previous: Option<&PaymentLeg> = None;
    for leg in legs {
        if leg.amount_msat == 0 || !ids.insert(&leg.leg_id) {
            return Err(SettlementError::Quote("zero or duplicate leg".into()));
        }
        if previous.is_some_and(|prior| {
            (&prior.amount_msat, &prior.role, &prior.recipient_pubkey)
                > (&leg.amount_msat, &leg.role, &leg.recipient_pubkey)
        }) {
            return Err(SettlementError::Quote(
                "legs are not canonically ordered".into(),
            ));
        }
        previous = Some(leg);
        sum = sum
            .checked_add(leg.amount_msat)
            .ok_or(SettlementError::Overflow)?;
    }
    if sum != gross_msat {
        return Err(SettlementError::Quote("leg sum differs from gross".into()));
    }
    Ok(())
}

pub fn aggregate_commitment(
    gross_msat: u64,
    legs: &[AggregateProofLeg],
) -> Result<String, SettlementError> {
    let mut ids = BTreeSet::new();
    let mut hashes = BTreeSet::new();
    let mut sum = 0_u64;
    for leg in legs {
        if leg.amount_msat == 0
            || leg.payment_hash.len() != 64
            || leg.proof_reference.is_empty()
            || !ids.insert(&leg.leg_id)
            || !hashes.insert(&leg.payment_hash)
        {
            return Err(SettlementError::Aggregate(
                "invalid, duplicate, or unproved leg".into(),
            ));
        }
        sum = sum
            .checked_add(leg.amount_msat)
            .ok_or(SettlementError::Overflow)?;
    }
    if sum != gross_msat {
        return Err(SettlementError::Aggregate(
            "leg sum differs from gross".into(),
        ));
    }
    canonical_commitment(&(SETTLEMENT_VERSION, gross_msat, legs))
}

fn canonical_commitment<T: Serialize>(value: &T) -> Result<String, SettlementError> {
    let bytes = serde_json::to_vec(value).map_err(|_| SettlementError::Serialization)?;
    Ok(hex::encode(Sha256::digest(bytes)))
}
