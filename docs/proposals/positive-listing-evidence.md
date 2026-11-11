# Proposal: positive listing evidence with partial relay coverage

**Status: non-normative, unagreed policy proposal.** This is not a protocol
consensus decision and does not relax ADP authorization, revocation or required
predecessor/completeness verification.

## Reference implementation behavior

Listing lookup considers signature-verified, well-formed candidates from
EOSE-completed relay responses before applying the minimum coverage threshold
used to conclude absence. One completed relay can therefore establish a listing
candidate; two successful relay completions remain required to conclude absence.
Events received without EOSE completion do not count. Multiple available valid
candidates still use the existing replaceable-event ordering and tie-breaks.

This behavior is limited to listing lookup. It is not a universal positive-
evidence policy and does not remove completeness requirements from collection,
authorization-chain, grant or receipt/revocation resolution.

## Availability and security trade-off

The policy improves availability when some configured relays are unreachable.
A valid signature establishes authorship and event integrity, **not freshness**.
An unavailable or withholding relay may hold a newer listing that changes the
authorized servers, payment destination or acquisition policy. Acting on the
best observed candidate can therefore act on superseded permissions or terms.
Snapshot pinning protects artifact consistency; it does not prove that a signed
snapshot is still the latest authorization or make every stale decision safe.

The trust assumption is that available completed responses provide acceptable
listing freshness for the operation. Selective withholding, eclipse conditions
and correlated relay failures can violate that assumption. Broader adoption
needs explicit operation-specific policy, not an unconditional claim that
availability is always the safer outcome.

## Revocation and normative compatibility

Keep historical authorization rules, required predecessors, fork rejection,
revocation checks and indeterminate handling unchanged. Missing revocation
evidence is not proof of no revocation. A listing's presence cannot substitute
for a complete required authorization or status chain.

**Unresolved compatibility issue:** if an operation's agreed normative freshness
or evidence-completeness requirement requires fuller listing coverage, selecting
a candidate under this policy is not established as conformant. The implementation
does not amend that requirement. Applicability and mitigations require review;
the specification must not be weakened merely to justify existing code.

Possible review topics include operation-specific quorum/freshness requirements,
configured relay independence, bounded stale-read policies, eclipse resistance,
revocation discoverability, and fail-closed behavior for irreversible actions.
