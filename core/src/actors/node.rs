use std::{io, path::PathBuf, process::exit, result::Result};

use actix::{Actor, System};
use log::info;

use crate::actors::{
    blocks_manager::BlocksManager, config_manager::ConfigManager,
    connections_manager::ConnectionsManager, epoch_manager::EpochManager,
    inventory_manager::InventoryManager, json_rpc::JsonRpcServer, mempool_manager::MempoolManager,
    peers_manager::PeersManager, sessions_manager::SessionsManager,
    storage_manager::StorageManager, utxo_manager::UtxoManager,
};

/// Function to run the main system
pub fn run(config: Option<PathBuf>, callback: fn()) -> Result<(), io::Error> {
    // Init system
    let system = System::new("node");

    // Call cb function (register interrupt handlers)
    callback();

    // Start config manager actor
    let config_manager_addr = ConfigManager::new(config).start();
    System::current().registry().set(config_manager_addr);

    // Start storage manager actor
    let storage_manager_addr = StorageManager::default().start();
    System::current().registry().set(storage_manager_addr);

    // Start peers manager actor
    let peers_manager_addr = PeersManager::default().start();
    System::current().registry().set(peers_manager_addr);

    // Start connections manager actor
    let connections_manager_addr = ConnectionsManager::default().start();
    System::current().registry().set(connections_manager_addr);

    // Start session manager actor
    let sessions_manager_addr = SessionsManager::default().start();
    System::current().registry().set(sessions_manager_addr);

    // Start epoch manager actor
    let epoch_manager_addr = EpochManager::default().start();
    System::current().registry().set(epoch_manager_addr);

    // Start blocks manager actor
    let blocks_manager_addr = BlocksManager::default().start();
    System::current().registry().set(blocks_manager_addr);

    // Start mempool manager actor
    let mempool_manager_addr = MempoolManager::start_default();
    System::current().registry().set(mempool_manager_addr);

    // Start UTXO manager actor
    let utxo_manager_addr = UtxoManager::start_default();
    System::current().registry().set(utxo_manager_addr);

    // Start inventory manager actor
    let inventory_manager_addr = InventoryManager::start_default();
    System::current().registry().set(inventory_manager_addr);

    // Start JSON RPC server (this doesn't need to be in the registry)
    let _json_rpc_server_addr = JsonRpcServer::default().start();

    // Run system
    system.run();

    Ok(())
}

/// Function to close the main system
pub fn close() {
    info!("Closing node");

    // FIXME(#72): find out how to gracefully stop the system
    // System::current().stop();

    // Process exit
    exit(0);
}
