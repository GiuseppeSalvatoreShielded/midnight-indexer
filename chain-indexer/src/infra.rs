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

pub mod storage;
pub mod subxt_node;

#[cfg(feature = "cloud")]
use indexer_common::infra::{ledger_state_storage, pool, pub_sub};

#[cfg(feature = "cloud")]
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Config {
    #[serde(rename = "storage")]
    pub storage_config: pool::postgres::Config,

    #[serde(rename = "pub_sub")]
    pub pub_sub_config: pub_sub::nats::Config,

    #[serde(rename = "ledger_state_storage")]
    pub ledger_state_storage_config: ledger_state_storage::nats::Config,

    #[serde(rename = "node")]
    pub node_config: subxt_node::Config,
}
