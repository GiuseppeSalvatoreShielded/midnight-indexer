// This file is part of midnight-indexer.
// Copyright (C) 2025 Midnight Foundation
// SPDX-License-Identifier: Apache-2.0
// Licensed under the Apache License, Version 2.0 (the "License");
// You may not use this file except in compliance with the License.
// You may obtain a copy of the License at
// http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::{
    domain::{Transaction, Wallet, storage::Storage},
    infra::storage::{self},
};
use chacha20poly1305::ChaCha20Poly1305;
use futures::TryStreamExt;
use indexer_common::{domain::ViewingKey, infra::pool::sqlite::SqlitePool};
use indoc::indoc;
use sqlx::{QueryBuilder, Row, Sqlite, types::time::OffsetDateTime};
use std::{num::NonZeroUsize, time::Duration};
use uuid::Uuid;

type Tx = sqlx::Transaction<'static, Sqlite>;

/// Sqlite based implementation of [Storage].
#[derive(Clone)]
pub struct SqliteStorage {
    cipher: ChaCha20Poly1305,
    pool: SqlitePool,
}

impl SqliteStorage {
    /// Create a new [SqliteStorage].
    pub fn new(cipher: ChaCha20Poly1305, pool: SqlitePool) -> Self {
        Self { cipher, pool }
    }
}

impl Storage for SqliteStorage {
    type Database = sqlx::Sqlite;

    async fn acquire_lock(&mut self, _wallet_id: Uuid) -> Result<Option<Tx>, sqlx::Error> {
        // SQLite doesn't support advisory locks like PostgreSQL. But in standalone mode (single
        // instance) we need not exclude other, i.e. "locking" is always successful.
        let tx = self.pool.begin().await?;
        Ok(Some(tx))
    }

    async fn get_transactions(
        &self,
        from: u64,
        limit: NonZeroUsize,
        tx: &mut Tx,
    ) -> Result<Vec<Transaction>, sqlx::Error> {
        let query = indoc! {"
            SELECT id, protocol_version, raw
            FROM transactions
            WHERE id >= $1
            ORDER BY id
            LIMIT $2
        "};

        sqlx::query_as(query)
            .bind(from as i64)
            .bind(limit.get() as i32)
            .fetch_all(&mut **tx)
            .await
    }

    async fn save_relevant_transactions(
        &self,
        viewing_key: &ViewingKey,
        transactions: &[Transaction],
        last_indexed_transaction_id: u64,
        tx: &mut Tx,
    ) -> Result<(), sqlx::Error> {
        let id = Uuid::now_v7();
        let session_id = viewing_key.to_session_id();
        let viewing_key = viewing_key
            .encrypt(id, &self.cipher)
            .map_err(|error| sqlx::Error::Encode(error.into()))?;

        let query = indoc! {"
            INSERT INTO wallets (
                id,
                session_id,
                viewing_key,
                last_indexed_transaction_id,
                last_active
            )
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (session_id)
            DO UPDATE SET last_indexed_transaction_id = $4
            RETURNING id
        "};

        let wallet_id = sqlx::query(query)
            .bind(id)
            .bind(session_id.as_ref())
            .bind(viewing_key)
            .bind(last_indexed_transaction_id as i64)
            .bind(OffsetDateTime::now_utc())
            .fetch_one(&mut **tx)
            .await?
            .try_get::<Uuid, _>("id")?;

        if !transactions.is_empty() {
            let query = indoc! {"
                INSERT INTO relevant_transactions (wallet_id, transaction_id)
            "};
            QueryBuilder::new(query)
                .push_values(transactions, |mut q, transaction| {
                    q.push_bind(wallet_id).push_bind(transaction.id as i64);
                })
                .build()
                .execute(&mut **tx)
                .await?;
        }

        Ok(())
    }

    async fn active_wallets(&self, ttl: Duration) -> Result<Vec<Uuid>, sqlx::Error> {
        let mut tx = self.pool.begin().await?;

        // Query wallets.
        let query = indoc! {"
            SELECT id, last_active
            FROM wallets
            WHERE active = TRUE
        "};

        let wallets = sqlx::query_as::<_, (Uuid, OffsetDateTime)>(query)
            .fetch(&mut *tx)
            .try_collect::<Vec<_>>()
            .await?;

        // Mark inactive wallets.
        let now = OffsetDateTime::now_utc();
        let outdated_ids = wallets
            .iter()
            .filter_map(|&(id, last_active)| (now - last_active > ttl).then_some(id));
        for id in outdated_ids {
            let query = indoc! {"
                UPDATE wallets
                SET active = FALSE
                WHERE id = ?
            "};

            sqlx::query(query).bind(id).execute(&mut *tx).await?;
        }

        // Return active wallet IDs.
        let ids = wallets
            .into_iter()
            .filter_map(|(id, last_active)| (now - last_active <= ttl).then_some(id))
            .collect::<Vec<_>>();
        Ok(ids)
    }

    async fn get_wallet_by_id(&self, id: Uuid, tx: &mut Tx) -> Result<Wallet, sqlx::Error> {
        let query = indoc! {"
            SELECT id, viewing_key, last_indexed_transaction_id
            FROM wallets
            WHERE id = $1
        "};

        let wallet = sqlx::query_as::<_, storage::Wallet>(query)
            .bind(id)
            .fetch_one(&mut **tx)
            .await?;

        Wallet::try_from((wallet, &self.cipher)).map_err(|error| sqlx::Error::Decode(error.into()))
    }
}

#[cfg(test)]
mod tests {
    use crate::{domain::storage::Storage, infra::storage::sqlite::SqliteStorage};
    use chacha20poly1305::{ChaCha20Poly1305, Key, KeyInit};
    use futures::{StreamExt, TryStreamExt};
    use indexer_common::{
        domain::{TransactionResult, ViewingKey},
        infra::{
            migrations,
            pool::{self, sqlite::SqlitePool},
        },
    };
    use indoc::indoc;
    use sqlx::{
        QueryBuilder, Row,
        types::{Json, time::OffsetDateTime},
    };
    use std::{error::Error as StdError, iter, time::Duration};
    use uuid::Uuid;

    #[tokio::test]
    async fn test_storage() -> Result<(), Box<dyn StdError>> {
        let pool = SqlitePool::new(pool::sqlite::Config::default()).await?;

        migrations::sqlite::run(&pool).await?;

        // Fill DB with test data.

        let query = indoc! {"
            INSERT INTO blocks (
                hash,
                height,
                protocol_version,
                parent_hash,
                timestamp
            ) 
        "};
        let block_id = QueryBuilder::new(query)
            .push_values(iter::once(1), |mut q, id| {
                q.push_bind(id.to_string())
                    .push_bind(id)
                    .push_bind(1_000)
                    .push_bind(0)
                    .push_bind(0);
            })
            .push(" RETURNING id")
            .build()
            .fetch_one(&*pool)
            .await?
            .try_get::<i64, _>("id")?;

        let ids = 1..=100;
        let query = indoc! {"
            INSERT INTO transactions (
                block_id,
                hash,
                protocol_version,
                transaction_result,
                raw,
                merkle_tree_root,
                start_index,
                end_index
            )
        "};
        let transaction_ids = QueryBuilder::new(query)
            .push_values(ids, |mut q, id| {
                q.push_bind(block_id)
                    .push_bind(id.to_string())
                    .push_bind(1_000)
                    .push_bind(Json(TransactionResult::Success))
                    .push_bind("raw")
                    .push_bind("merkle_tree_root")
                    .push_bind(0)
                    .push_bind(1);
            })
            .push(" RETURNING id")
            .build()
            .fetch(&*pool)
            .map(|row| row.and_then(|row| row.try_get::<i64, _>("id")))
            .try_collect::<Vec<_>>()
            .await?;

        for transaction_id in transaction_ids {
            let query = indoc! {"
            INSERT INTO transaction_identifiers (transaction_id, identifier)
        "};
            QueryBuilder::new(query)
                .push_values(iter::once("identifier"), |mut q, identifier| {
                    q.push_bind(transaction_id).push_bind(identifier);
                })
                .build()
                .execute(&*pool)
                .await?;
        }

        let cipher =
            ChaCha20Poly1305::new(&Key::clone_from_slice(b"01234567890123456789012345678901"));

        let viewing_key_a = ViewingKey::from([0; 32]);
        let viewing_key_b = ViewingKey::from([1; 32]);
        let session_id_a = viewing_key_a.to_session_id();
        let session_id_b = viewing_key_b.to_session_id();

        let uuid_a = Uuid::now_v7();
        let encrypted_viewing_key_a = viewing_key_a.encrypt(uuid_a, &cipher)?;
        let uuid_b = Uuid::now_v7();
        let encrypted_viewing_key_b = viewing_key_b.encrypt(uuid_b, &cipher)?;

        let wallets = [
            (uuid_a, encrypted_viewing_key_a, session_id_a, 1),
            (uuid_b, encrypted_viewing_key_b, session_id_b, 42),
        ];

        let query = indoc! {"
            INSERT INTO wallets (
                id,
                session_id,
                viewing_key,
                last_indexed_transaction_id,
                last_active
            )
        "};
        QueryBuilder::new(query)
            .push_values(
                wallets,
                |mut q, (id, viewing_key, session_id, last_indexed_transaction_id)| {
                    q.push_bind(id)
                        .push_bind(session_id.as_ref().to_owned())
                        .push_bind(viewing_key)
                        .push_bind(last_indexed_transaction_id)
                        .push_bind(OffsetDateTime::now_utc());
                },
            )
            .build()
            .execute(&*pool)
            .await?;

        // Start the actual testing.

        let mut storage = SqliteStorage::new(cipher, pool);

        let active_wallets = storage.active_wallets(Duration::from_secs(60)).await?;
        assert_eq!(active_wallets, [uuid_a, uuid_b]);

        let tx = storage.acquire_lock(uuid_b).await?;
        assert!(tx.is_some());
        let mut tx = tx.unwrap();

        let transactions = storage
            .get_transactions(42, 10.try_into()?, &mut tx)
            .await?;
        assert_eq!(
            transactions.iter().map(|t| t.id).collect::<Vec<_>>(),
            (42..52).collect::<Vec<_>>()
        );

        storage
            .save_relevant_transactions(&viewing_key_b, &transactions[0..5], 51, &mut tx)
            .await?;

        tx.commit().await?;

        let tx = storage.acquire_lock(uuid_b).await?;
        assert!(tx.is_some());
        let mut tx = tx.unwrap();

        let relevant_transactions = sqlx::query_as::<_, (Uuid, i64)>(
            "SELECT wallet_id, transaction_id
             FROM relevant_transactions",
        )
        .fetch_all(&mut *tx)
        .await?;

        assert_eq!(
            relevant_transactions,
            (42..47).map(|tx_id| (uuid_b, tx_id)).collect::<Vec<_>>()
        );

        Ok(())
    }
}
