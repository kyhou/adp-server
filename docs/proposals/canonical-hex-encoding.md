# Proposal: broader canonical hexadecimal encodings

**Status: non-normative, unagreed proposal.** This document does not amend
`ADP-01.md` or replace existing field-specific encoding requirements.

## Implemented scope

The experimental aggregate receipt parser currently requires each public
payment-proof reference to contain exactly 64 lowercase hexadecimal characters.
For `bolt11-preimage` legs, the reference must equal that leg's payment hash.
This is field-specific validation, not evidence that every hexadecimal field in
the server, clients, relays or databases rejects uppercase input.

## Proposed broader policy

Consider specifying a single lowercase encoding for fields explicitly defined
as hexadecimal, including artifact hashes, keys and event references. The
motivation is consistent identity: multiple spellings can disagree when used as
database keys, for deduplication or in byte-for-byte commitment comparisons.

One possible policy would reject noncanonical values received from another
participant rather than case-folding signed events, HTTP payloads or persisted
identities. Human-entered values could instead be normalized before validation
and signing. Bech32 identifiers and Lightning addresses would remain outside
this policy; their rules are not hexadecimal rules.

## Compatibility questions requiring review

- Inventory the existing normative rule and implemented parser for each field.
- Determine whether previously accepted uppercase or mixed-case values exist.
- Evaluate rejection versus normalization at each unsigned input boundary;
  never rewrite signed events or invalidate their signatures by normalization.
- Assess existing stored keys, artifact lookup, interoperability and downgrade
  behavior before introducing stricter rejection or any data migration.
- Align builders and parsers, publish adversarial fixtures, and agree rollout
  requirements before claiming universal enforcement.

Conventional encoders often emit lowercase already. That fact does not establish
compatibility for every existing consumer or authorize a blanket rejection rule.
