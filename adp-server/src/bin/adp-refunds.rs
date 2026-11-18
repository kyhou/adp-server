//! Operator CLI for partial-settlement refunds owed by this ADP Server operator.
//! Environment: ADP_SERVER_URL (this server's public origin), ADP_OPERATOR_NSEC.
//! It never pays; see `adp_server::operator_refunds`.
use adp_server::operator_refunds::{render_obligations, OperatorRefundClient, PAY_ONCE_WARNING};

const USAGE: &str = "usage: adp-refunds list | prepare <quote_id> <leg_id> | confirm <quote_id> <leg_id> <attempt_id> <preimage-hex>";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let server = std::env::var("ADP_SERVER_URL")
        .map_err(|_| anyhow::anyhow!("ADP_SERVER_URL is required"))?;
    let keys = nostr_sdk::Keys::parse(
        &std::env::var("ADP_OPERATOR_NSEC")
            .map_err(|_| anyhow::anyhow!("ADP_OPERATOR_NSEC is required"))?,
    )?;
    let client = OperatorRefundClient::new(&server, keys)?;
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["list"] => {
            println!("operator {}", client.operator_pubkey());
            print!("{}", render_obligations(&client.obligations().await?));
        }
        ["prepare", quote, leg] => {
            let invoice = client.prepare(quote, leg).await?;
            println!(
                "attempt_id  {}",
                invoice["attempt_id"].as_str().unwrap_or("?")
            );
            println!("amount_msat {}", invoice["amount_msat"]);
            println!("expires_at  {}", invoice["expires_at"]);
            println!("bolt11      {}", invoice["bolt11"].as_str().unwrap_or("?"));
            println!("{PAY_ONCE_WARNING}");
        }
        ["confirm", quote, leg, attempt, preimage] => {
            let result = client.confirm(quote, leg, attempt, preimage).await?;
            println!("refund state: {}", result["state"].as_str().unwrap_or("?"));
        }
        _ => anyhow::bail!(USAGE),
    }
    Ok(())
}
