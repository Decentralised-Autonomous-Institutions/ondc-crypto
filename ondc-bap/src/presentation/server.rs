//! Main server implementation for ONDC BAP

use std::net::SocketAddr;
use std::sync::Arc;
use tokio::signal;
use tracing::{error, info, warn};
use axum_server::tls_rustls::RustlsConfig;

use super::routes::create_router;
use crate::config::load_config;
use crate::services::{KeyManagementService, RegistryClient};
use crate::{BAPConfig, Result};

/// Main BAP server implementation
pub struct BAPServer {
    config: Arc<BAPConfig>,
    key_manager: Arc<KeyManagementService>,
    registry_client: Arc<RegistryClient>,
}

impl BAPServer {
    /// Create a new BAP server instance
    pub async fn new() -> Result<Self> {
        let config = Arc::new(load_config()?);
        info!("BAP Server configuration loaded successfully");

        let key_manager = Arc::new(
            KeyManagementService::new(config.keys.clone())
                .await
                .map_err(|e| crate::error::AppError::Internal(e.to_string()))?,
        );
        info!("Key management service initialized successfully");

        let registry_client = Arc::new(RegistryClient::new(key_manager.clone(), config.ondc.clone())?);
        info!("Registry client initialized successfully");

        Ok(Self {
            config,
            key_manager,
            registry_client,
        })
    }

    /// Run the server
    pub async fn run(&self) -> Result<()> {
        let addr = SocketAddr::from((
            self.config
                .server
                .host
                .parse::<std::net::IpAddr>()
                .unwrap_or_else(|_| [0, 0, 0, 0].into()),
            self.config.server.port,
        ));

        info!("Starting BAP Server on {}", addr);

        // Create router
        let app = create_router(self.config.clone(), self.key_manager.clone(), self.registry_client.clone());

        // Check if TLS certificates are available
        let cert_path = "/opt/ssl-certs/fullchain.pem";
        let key_path = "/opt/ssl-certs/privkey.pem";
        
        if std::path::Path::new(cert_path).exists() && std::path::Path::new(key_path).exists() {
            info!("TLS certificates found, starting HTTPS server");
            self.run_https_server(addr, app, cert_path, key_path).await
        } else {
            warn!("TLS certificates not found, starting HTTP server");
            self.run_http_server(addr, app).await
        }
    }

    /// Run HTTP server (fallback when TLS certificates are not available)
    async fn run_http_server(&self, addr: SocketAddr, app: axum::Router) -> Result<()> {
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| crate::error::AppError::Internal(e.to_string()))?;

        info!("HTTP Server listening on {}", addr);

        let graceful = axum::serve(listener, app).with_graceful_shutdown(shutdown_signal());

        if let Err(e) = graceful.await {
            error!("Server error: {}", e);
            return Err(crate::error::AppError::Internal(e.to_string()));
        }

        info!("Server shutdown complete");
        Ok(())
    }

    /// Run HTTPS server with TLS support
    async fn run_https_server(&self, addr: SocketAddr, app: axum::Router, cert_path: &str, key_path: &str) -> Result<()> {
        let tls_config = self.load_rustls_config(cert_path, key_path).await?;

        info!("HTTPS Server listening on {}", addr);

        let server = axum_server::bind_rustls(addr, tls_config);
        let graceful = server.serve(app.into_make_service());
        
        tokio::select! {
            result = graceful => {
                if let Err(e) = result {
                    error!("Server error: {}", e);
                    return Err(crate::error::AppError::Internal(e.to_string()));
                }
            }
            _ = shutdown_signal() => {
                info!("Shutdown signal received, server shutting down gracefully");
            }
        }

        info!("Server shutdown complete");
        Ok(())
    }

    /// Load TLS configuration from certificate and key files
    async fn load_rustls_config(&self, cert_path: &str, key_path: &str) -> Result<RustlsConfig> {
        // Read certificate and key files as bytes
        let cert_pem = std::fs::read(cert_path)
            .map_err(|e| crate::error::AppError::Internal(format!("Failed to read certificate file: {}", e)))?;
        let key_pem = std::fs::read(key_path)
            .map_err(|e| crate::error::AppError::Internal(format!("Failed to read private key file: {}", e)))?;

        // Create TLS configuration using axum-server's RustlsConfig
        let config = RustlsConfig::from_pem(cert_pem, key_pem)
            .await
            .map_err(|e| crate::error::AppError::Internal(format!("Failed to create TLS config: {}", e)))?;

        Ok(config)
    }

    /// Get server configuration
    pub fn config(&self) -> &BAPConfig {
        &self.config
    }

    /// Get key manager
    pub fn key_manager(&self) -> &KeyManagementService {
        &self.key_manager
    }
}

/// Handle graceful shutdown signals
async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("Failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {
            info!("Received Ctrl+C, shutting down gracefully");
        },
        _ = terminate => {
            info!("Received SIGTERM, shutting down gracefully");
        },
    }

    info!("Shutdown signal received, starting graceful shutdown");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_server_creation() {
        // This test will need proper configuration setup
        // For now, just verify the struct can be created with test config
        let test_config = crate::config::environment::create_test_config();
        let key_manager = KeyManagementService::new(test_config.keys.clone())
            .await
            .unwrap();
        let key_manager_arc = Arc::new(key_manager);
        let registry_client = RegistryClient::new(key_manager_arc.clone(), test_config.ondc.clone())
            .unwrap();

        let _server = BAPServer {
            config: Arc::new(test_config),
            key_manager: key_manager_arc,
            registry_client: Arc::new(registry_client),
        };
    }
}
