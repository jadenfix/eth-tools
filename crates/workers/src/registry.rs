//! Bounded Base identity-registry indexer. Event writes and cursor commit atomically.
use crate::{WorkerContext, WorkerSummary};
use alloy::{rpc::types::Filter, sol_types::SolEvent};
use alloy_primitives::Address;
use bigdecimal::BigDecimal;
use eth_tools_core::{Registered, Transfer, URIUpdated, REGISTERED_TOPIC, TRANSFER_TOPIC, URI_UPDATED_TOPIC};
use eth_tools_db::cursors;
use eth_tools_rpc::{confirmed_head, MAX_LOGS_PER_CALL, REORG_SAFETY_BLOCKS};
use std::str::FromStr;
use vercel_runtime::Error;

pub async fn run(context: WorkerContext) -> Result<WorkerSummary, Error> {
    let chain = eth_tools_core::chains::by_name("base").ok_or("Base configuration missing")?;
    if context
        .rpc
        .get_chain_id()
        .await
        .map_err(|_| Error::from("RPC chain identity unavailable"))?
        != chain.chain_id
    {
        return Err(Error::from("RPC endpoint is not Base; no cursor advanced"));
    }
    let head = context
        .rpc
        .get_block_number()
        .await
        .map_err(|_| Error::from("RPC head unavailable"))?;
    let safe = confirmed_head(head, REORG_SAFETY_BLOCKS);
    let key = format!("identity:{}", chain.chain_id);
    let mut transaction = context.pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
        .bind(&key)
        .execute(&mut *transaction)
        .await?;
    let cursor = cursors::get(&mut transaction, &key).await?;
    let start = cursor
        .map(|c| u64::try_from(c.last_block).unwrap_or(0).saturating_add(1))
        .unwrap_or(chain.genesis_block);
    let mut summary = WorkerSummary::ok("registry_scraper");
    summary.dryrun = context.dryrun;
    if start > safe {
        return Ok(summary);
    }
    let end = safe.min(start.saturating_add(MAX_LOGS_PER_CALL - 1));
    let filter = Filter::new()
        .address(Address::from(chain.identity_registry))
        .event_signature(vec![REGISTERED_TOPIC, URI_UPDATED_TOPIC, TRANSFER_TOPIC]);
    let mut logs = context
        .rpc
        .get_logs_paginated(filter, start, end, MAX_LOGS_PER_CALL)
        .await
        .map_err(|_| Error::from("RPC registry logs unavailable"))?;
    logs.sort_by_key(|log| (log.block_number, log.log_index));
    summary.rows_in = logs.len() as u64;
    let mut timestamps = std::collections::HashMap::new();
    for log in logs {
        if log.removed
            || log.block_number.is_none_or(|b| b < start || b > end)
            || log.log_index.is_none()
            || log.address() != Address::from(chain.identity_registry)
        {
            return Err(Error::from("invalid registry log provenance"));
        }
        let Some(topic) = log.topics().first() else {
            return Err(Error::from("registry event topic missing"));
        };
        if *topic == REGISTERED_TOPIC {
            let event = Registered::decode_log_data(&log.inner.data)
                .map_err(|_| Error::from("invalid Registered event"))?;
            let id = BigDecimal::from_str(&event.agentId.to_string())?;
            let block = log.block_number.ok_or("event block missing")?;
            let timestamp = match log.block_timestamp.or(timestamps.get(&block).copied()) {
                Some(time) => time,
                None => context
                    .rpc
                    .get_block_timestamp(block)
                    .await
                    .map_err(|_| Error::from("event block timestamp unavailable"))?,
            };
            timestamps.insert(block, timestamp);
            let timestamp = i64::try_from(timestamp)?;
            sqlx::query("INSERT INTO agents (chain_id,agent_id,owner,agent_uri,registered_at,updated_at) VALUES ($1,$2,$3,$4,to_timestamp($5::double precision),NOW()) ON CONFLICT (chain_id,agent_id) DO UPDATE SET owner=EXCLUDED.owner,agent_uri=EXCLUDED.agent_uri,updated_at=NOW()")
                .bind(chain.chain_id as i64).bind(id).bind(event.owner.as_slice()).bind(event.agentUri).bind(timestamp as f64).execute(&mut *transaction).await?;
        } else if *topic == URI_UPDATED_TOPIC {
            let event = URIUpdated::decode_log_data(&log.inner.data)
                .map_err(|_| Error::from("invalid URIUpdated event"))?;
            let result = sqlx::query(
                "UPDATE agents SET agent_uri=$3,updated_at=NOW() WHERE chain_id=$1 AND agent_id=$2",
            )
            .bind(chain.chain_id as i64)
            .bind(BigDecimal::from_str(&event.agentId.to_string())?)
            .bind(event.newUri)
            .execute(&mut *transaction)
            .await?;
            if result.rows_affected() != 1 {
                return Err(Error::from("URI update references an unindexed agent"));
            }
        } else if *topic == TRANSFER_TOPIC {
            let event = Transfer::decode_log_data(&log.inner.data)
                .map_err(|_| Error::from("invalid Transfer event"))?;
            if event.from != Address::ZERO {
                let result=sqlx::query("UPDATE agents SET owner=$3,agent_wallet=NULL,updated_at=NOW() WHERE chain_id=$1 AND agent_id=$2").bind(chain.chain_id as i64).bind(BigDecimal::from_str(&event.tokenId.to_string())?).bind(event.to.as_slice()).execute(&mut *transaction).await?;
                if result.rows_affected() != 1 {
                    return Err(Error::from("Transfer references an unindexed agent"));
                }
            }
        } else {
            return Err(Error::from("unexpected registry event"));
        }
        summary.rows_out += 1;
    }
    cursors::advance(&mut transaction, &key, i64::try_from(end)?, i32::MAX).await?;
    if context.dryrun {
        transaction.rollback().await?;
    } else {
        transaction.commit().await?;
    }
    Ok(summary)
}
