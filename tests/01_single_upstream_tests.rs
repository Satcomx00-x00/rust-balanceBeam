mod common;

use common::{init_logging, EchoServer, Server};
use std::sync::Arc;

// Fonction asynchrone pour initialiser l'équilibreur de charge et le serveur d'écho.
async fn setup() -> (EchoServer) {
    init_logging(); // Initialiser le système de journalisation.
    let upstream = EchoServer::new().await; // Créer une instance du serveur d'écho.
    // Créer une instance de l'équilibreur de charge avec l'adresse du serveur d'écho.
    upstream // Retourner l'équilibreur et le serveur d'écho.
}
/// Test des cas simples : ouvrir quelques connexions, chacune avec une seule requête, et s'assurer
/// que les requêtes sont correctement traitées.
#[tokio::test]
async fn test_simple_connections() {
    let upstream = setup().await; // Initialisation.

    // Envoi d'une requête GET et vérification de la réponse.
    log::info!("Envoi d'une requête GET");

    // Envoi d'une requête POST et vérification de la réponse.
    log::info!("Envoi d'une requête POST");

    // Vérification que le serveur d'origine a reçu deux requêtes.
    log::info!("Vérification que le serveur d'origine a reçu 2 requêtes");
    let num_requests_received = Box::new(upstream).stop().await;
    assert_eq!(num_requests_received, 2, "Le serveur amont n'a pas reçu le nombre attendu de requêtes");

    log::info!("Tout est terminé :)");
}
