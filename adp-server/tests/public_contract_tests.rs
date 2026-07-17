use adp_server::ownership::{AccessBasis, CredentialCheck, CredentialSource};
use adp_server::relay::RelayLookup;

fn access_basis_discriminant(value: AccessBasis) -> u8 {
    match value {
        AccessBasis::DownloadToken => 0,
        AccessBasis::EntitlementGrant => 1,
        AccessBasis::PurchaseReceipt => 2,
        AccessBasis::PublicPolicy => 3,
        AccessBasis::TimedAccessPolicy => 4,
    }
}

fn credential_source_discriminant(value: CredentialSource) -> u8 {
    match value {
        CredentialSource::Entitlements => 0,
        CredentialSource::Receipts => 1,
    }
}

fn credential_check_discriminant(value: CredentialCheck) -> u8 {
    match value {
        CredentialCheck::Valid(_) => 0,
        CredentialCheck::Absent => 1,
        CredentialCheck::Invalid { .. } => 2,
        CredentialCheck::Unavailable { .. } => 3,
    }
}

fn relay_lookup_discriminant(value: RelayLookup<usize>) -> u8 {
    match value {
        RelayLookup::Found(_) => 0,
        RelayLookup::ConfirmedAbsent { .. } => 1,
        RelayLookup::Unavailable { .. } => 2,
    }
}

#[test]
fn server_contracts_have_stable_variants_and_traits() {
    fn assert_contract<T: std::fmt::Debug + Clone + PartialEq + Eq>() {}

    assert_contract::<AccessBasis>();
    assert_contract::<CredentialSource>();
    assert_contract::<CredentialCheck>();
    assert_contract::<RelayLookup<usize>>();

    assert!(matches!(
        CredentialCheck::Valid(AccessBasis::EntitlementGrant),
        CredentialCheck::Valid(AccessBasis::EntitlementGrant)
    ));
    assert!(matches!(CredentialCheck::Absent, CredentialCheck::Absent));
    assert!(matches!(
        CredentialCheck::Invalid { candidate_count: 2 },
        CredentialCheck::Invalid { candidate_count: 2 }
    ));
    assert!(matches!(
        CredentialCheck::Unavailable {
            source: CredentialSource::Receipts
        },
        CredentialCheck::Unavailable {
            source: CredentialSource::Receipts
        }
    ));
    assert!(matches!(RelayLookup::Found(1usize), RelayLookup::Found(1)));
    assert!(matches!(
        RelayLookup::<usize>::ConfirmedAbsent {
            successful_relays: 2
        },
        RelayLookup::ConfirmedAbsent {
            successful_relays: 2
        }
    ));
    assert!(matches!(
        RelayLookup::<usize>::Unavailable {
            successful_relays: 1,
            failed_relays: 1
        },
        RelayLookup::Unavailable {
            successful_relays: 1,
            failed_relays: 1
        }
    ));

    let all_bases = [
        AccessBasis::DownloadToken,
        AccessBasis::EntitlementGrant,
        AccessBasis::PurchaseReceipt,
        AccessBasis::PublicPolicy,
        AccessBasis::TimedAccessPolicy,
    ];
    assert_eq!(all_bases.len(), 5);
    assert_ne!(CredentialSource::Entitlements, CredentialSource::Receipts);
    assert_eq!(access_basis_discriminant(AccessBasis::DownloadToken), 0);
    assert_eq!(
        credential_source_discriminant(CredentialSource::Receipts),
        1
    );
    assert_eq!(credential_check_discriminant(CredentialCheck::Absent), 1);
    assert_eq!(relay_lookup_discriminant(RelayLookup::Found(1)), 0);
}
