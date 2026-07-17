#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessBasis {
    DownloadToken,
    EntitlementGrant,
    PurchaseReceipt,
    PublicPolicy,
    TimedAccessPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialSource {
    Entitlements,
    Receipts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialCheck {
    Valid(AccessBasis),
    Absent,
    Invalid { candidate_count: usize },
    Unavailable { source: CredentialSource },
}
