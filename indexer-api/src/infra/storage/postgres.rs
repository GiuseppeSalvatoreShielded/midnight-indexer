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

mod block;
mod contract_action;
mod transaction;
mod unshielded;
mod wallet;

use crate::domain::storage::Storage;
use chacha20poly1305::ChaCha20Poly1305;
use derive_more::Debug;
use indexer_common::infra::pool::postgres::PostgresPool;
use log::debug;

/// Postgres based implementation of [Storage].
#[derive(Debug, Clone)]
pub struct PostgresStorage {
    #[debug(skip)]
    cipher: ChaCha20Poly1305,
    pool: PostgresPool,
}

impl PostgresStorage {
    /// Create a new [PostgresStorage].
    pub fn new(cipher: ChaCha20Poly1305, pool: PostgresPool) -> Self {
        Self { cipher, pool }
    }
}

impl Storage for PostgresStorage {}
