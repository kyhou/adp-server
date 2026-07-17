use adp_server::ownership::{AccessBasis, CredentialCheck, CredentialSource};
use adp_server::relay::RelayLookup;

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
}
