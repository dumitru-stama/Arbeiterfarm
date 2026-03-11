use crate::providers::gmail::GmailProvider;
use crate::providers::protonmail::ProtonMailProvider;
use crate::providers::EmailProvider;
use crate::recipient_rules;
use crate::types::EmailMessage;
use sqlx::PgPool;

/// Process all due scheduled emails. Called from `af tick`.
/// Returns the number of successfully sent emails.
pub async fn process_due_emails(pool: &PgPool) -> anyhow::Result<u64> {
    let due = af_db::email::list_due_scheduled(pool, 50).await?;
    if due.is_empty() {
        return Ok(0);
    }

    let gmail = GmailProvider::new();
    let protonmail = ProtonMailProvider::new();
    let mut sent_count = 0u64;

    for scheduled in &due {
        // Atomically claim — skip if already claimed by another tick
        let claimed = match af_db::email::claim_scheduled(pool, scheduled.id).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(scheduled_id = %scheduled.id, error = %e, "failed to claim scheduled email");
                continue;
            }
        };
        if !claimed {
            continue;
        }

        // Load credentials for the scheduled email's user
        let cred = match scheduled.user_id {
            Some(uid) => match af_db::email::get_default_credential(pool, uid, Some(&scheduled.provider)).await {
                Ok(c) => c,
                Err(e) => {
                    let err = format!("failed to load credentials: {e}");
                    let _ = af_db::email::fail_scheduled(pool, scheduled.id, &err).await;
                    log_scheduled_outcome(pool, scheduled, false, Some(&err), None).await;
                    continue;
                }
            },
            None => None,
        };

        let cred = match cred {
            Some(c) => c,
            None => {
                let err = "no credentials found for scheduled email";
                let _ = af_db::email::fail_scheduled(pool, scheduled.id, err).await;
                log_scheduled_outcome(pool, scheduled, false, Some(err), None).await;
                continue;
            }
        };

        let provider: &dyn EmailProvider = match scheduled.provider.as_str() {
            "gmail" => &gmail,
            "protonmail" => &protonmail,
            _ => {
                let err = format!("unknown provider '{}'", scheduled.provider);
                let _ = af_db::email::fail_scheduled(pool, scheduled.id, &err).await;
                log_scheduled_outcome(pool, scheduled, false, Some(&err), None).await;
                continue;
            }
        };

        let to: Vec<String> = scheduled
            .to_addresses
            .as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
            .unwrap_or_default();
        let cc: Vec<String> = scheduled
            .cc_addresses
            .as_ref()
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
            .unwrap_or_default();
        let bcc: Vec<String> = scheduled
            .bcc_addresses
            .as_ref()
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
            .unwrap_or_default();

        // Re-check recipient rules — rules may have changed since scheduling time
        match af_db::email::list_recipient_rules(pool, None, Some(scheduled.project_id)).await {
            Ok(rules) => {
                if let Err(reason) = recipient_rules::evaluate_all_recipients(&to, &cc, &bcc, &rules) {
                    let err = format!("recipient blocked at send time: {reason}");
                    tracing::warn!(scheduled_id = %scheduled.id, "{err}");
                    let _ = af_db::email::fail_scheduled(pool, scheduled.id, &err).await;
                    log_scheduled_outcome(pool, scheduled, false, Some(&err), None).await;
                    continue;
                }
            }
            Err(e) => {
                // Fail-closed: cannot load rules = reject send for safety
                let err = format!("failed to load recipient rules: {e}");
                tracing::error!(scheduled_id = %scheduled.id, "{err}");
                let _ = af_db::email::fail_scheduled(pool, scheduled.id, &err).await;
                log_scheduled_outcome(pool, scheduled, false, Some(&err), None).await;
                continue;
            }
        }

        let msg = EmailMessage {
            from: scheduled.from_address.clone(),
            to,
            cc,
            bcc,
            subject: scheduled.subject.clone(),
            body_text: scheduled.body_text.clone().unwrap_or_default(),
            body_html: scheduled.body_html.clone(),
            in_reply_to: None,
            references: None,
            thread_id: None,
        };

        match provider.send(&msg, &cred.credentials_json).await {
            Ok(result) => {
                if let Err(e) = af_db::email::complete_scheduled(pool, scheduled.id, Some(&result.provider_message_id)).await {
                    tracing::warn!(scheduled_id = %scheduled.id, error = %e, "failed to mark scheduled email as complete");
                }
                log_scheduled_outcome(
                    pool,
                    scheduled,
                    true,
                    None,
                    Some(&result.provider_message_id),
                )
                .await;
                sent_count += 1;
            }
            Err(e) => {
                let err_msg = e.to_string();
                tracing::warn!(
                    scheduled_id = %scheduled.id,
                    attempt = scheduled.attempt_count + 1,
                    error = %err_msg,
                    "scheduled email send failed"
                );
                if let Err(db_err) = af_db::email::fail_scheduled(pool, scheduled.id, &err_msg).await {
                    tracing::warn!(scheduled_id = %scheduled.id, error = %db_err, "failed to mark scheduled email as failed");
                }
                log_scheduled_outcome(pool, scheduled, false, Some(&err_msg), None).await;
            }
        }
    }

    Ok(sent_count)
}

async fn log_scheduled_outcome(
    pool: &PgPool,
    scheduled: &af_db::email::EmailScheduledRow,
    success: bool,
    error: Option<&str>,
    provider_msg_id: Option<&str>,
) {
    let action = if success {
        "scheduled_send"
    } else {
        "scheduled_fail"
    };

    if let Err(e) = af_db::email::insert_email_log(
        pool,
        Some(scheduled.project_id),
        scheduled.user_id,
        action,
        &scheduled.provider,
        Some(&scheduled.from_address),
        Some(&scheduled.to_addresses),
        Some(&scheduled.subject),
        scheduled.tone.as_deref(),
        success,
        error,
        provider_msg_id,
        Some(scheduled.id),
        None,
        scheduled.thread_id,
        None,
    )
    .await
    {
        tracing::warn!("failed to write scheduled email log: {e}");
    }
}
