// On regroupe ici des modules et des trucs utiles.
mod request;
mod response;

// Pour gérer les arguments de la ligne de commande.
use clap::Parser;
use rand::{Rng, SeedableRng};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
// use tokio::time;
use tokio::net::TcpListener;
use tokio::{net::TcpStream, sync::RwLock};
use tokio::sync::Mutex;
use tokio::time::delay_for;
use tokio::time::Duration;
use tokio::io::ErrorKind;
use tokio::stream::StreamExt;
// use clap::{Arg, Command};


/// Options de la ligne de commande pour l'équilibrage de charge avec Loadbalancebeam.
#[derive(Parser, Debug)]
#[clap(about = "S'amuser avec l'équilibrage de charge")]
struct CmdOptions {
    #[clap(short, long, help = "Adresse IP et port d'écoute", default_value = "0.0.0.0:1100")]
    bind: String, // Où écouter pour les connexions entrantes.
    
    #[clap(short, long, help = "Serveurs amont pour rediriger les requêtes")]
    upstream: Vec<String>, // Les serveurs vers lesquels envoyer les requêtes.
    
    #[clap(long, help = "Intervalle de vérification de l'état des serveurs amont (en secondes)", default_value = "10")]
    active_health_check_interval: usize, // Combien de temps entre les vérifications d'état.
    
    #[clap(long, help = "Chemin pour envoyer la requête de vérification d'état", default_value = "/")]
    active_health_check_path: String, // Où envoyer la requête de vérif.
    
    #[clap(long, help = "Nombre maximal de requêtes acceptées par IP par minute (0 = illimité)", default_value = "0")]
    max_requests_per_minute: usize, // Limite de requêtes par minute par IP.
}


// État du proxy, genre quels serveurs sont ok, lesquels sont en rade, combien de requêtes on laisse passer, etc.
struct ProxyState {
    // Toutes les combien on vérifie si les serveurs amont sont toujours debout. Pour plus tard.
    active_health_check_interval: usize,
    // Où envoyer les requêtes de vérif pour voir si les serveurs amont répondent. Aussi pour plus tard.
    active_health_check_path: String,
    // Combien de requêtes un IP peut faire dans une minute. Pour encore plus tard.
    max_requests_per_minute: usize,
    // Les adresses des serveurs par lesquels on fait passer les requêtes.
    upstream_addresses: Vec<String>,

    // Si les serveurs amont sont ok ou pas, avec un verrou pour lire/écrire proprement.
    upstream_status: RwLock<(usize, Vec<bool>)>,
    // Compteur pour la limite de requête, protégé par un Mutex.
    rate_limit_counter: Mutex<HashMap<IpAddr, usize>>,
}
/// Point d'entrée principal du programme Loadbalancebeam.
#[tokio::main]
async fn main() {
    // Configuration des logs.
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "debug");
    }
    pretty_env_logger::init();

    // Analyse des arguments de la ligne de commande.
    let options = CmdOptions::parse();
    if options.upstream.is_empty() {
        log::error!("Au moins un serveur amont est requis avec --upstream.");
        std::process::exit(1);
    }

    // Initialisation de l'écoute des connexions.
    let mut listener = match TcpListener::bind(&options.bind).await {
        Ok(listener) => listener,
        Err(err) => {
            log::error!("Impossible de se lier à {}: {}", options.bind, err);
            std::process::exit(1);
        }
    };
    log::info!("Écoute sur {}", options.bind);

    let nb_upstream = options.upstream.len();
    // Préparation de l'état partagé entre les tâches.
    let etat = ProxyState {
        upstream_addresses: options.upstream,
        active_health_check_interval: options.active_health_check_interval,
        active_health_check_path: options.active_health_check_path,
        max_requests_per_minute: options.max_requests_per_minute,
        upstream_status: RwLock::new((nb_upstream, vec![true; nb_upstream])),
        rate_limit_counter: Mutex::new(HashMap::new()),
    };
    let etat_partagé = Arc::new(etat);

    // Lancement de la vérification de l'état des serveurs amont en asynchrone.
    let etat_clone_0 = etat_partagé.clone();
    tokio::spawn(async move {
        active_health_check(etat_clone_0).await;
    });

    // Si une limite de requêtes est définie, lance le compteur en asynchrone.
    if etat_partagé.max_requests_per_minute > 0 {
        let etat_clone_1 = etat_partagé.clone();
        tokio::spawn(async move {
            rate_limit_counter_refresher(etat_clone_1, 60).await;
        });
    }

    // Gestion des connexions entrantes.
    while let Some(stream) = listener.next().await {
        match stream {
            Ok(mut stream) => {
                if etat_partagé.max_requests_per_minute > 0 {
                    let mut compteur = etat_partagé.rate_limit_counter.lock().await;
                    let ip = stream.peer_addr().unwrap().ip();
                    let compte = compteur.entry(ip).or_insert(0);
                    log::debug!("IP: {}, Compte: {}", ip, compte);
                    *compte += 1;
                    if *compte > etat_partagé.max_requests_per_minute {
                        // Bloque le client si la limite est atteinte.
                        let réponse = response::make_http_error(http::StatusCode::TOO_MANY_REQUESTS);
                        response::write_to_stream(&réponse, &mut stream).await.unwrap();
                        continue;
                    }
                }
                let etat_clone = etat_partagé.clone();
                tokio::spawn(async move {
                    // Traitement de la connexion.
                    handle_connection(stream, etat_clone).await;
                });
            }
            Err(_) => break, // Arrête tout en cas d'erreur.
        }
    }
}
/// Réinitialise le compteur de limite de taux toutes les x secondes.
async fn rate_limit_counter_refresher(state: Arc<ProxyState>, interval: u64) {
    // Attend le délai spécifié.
    delay_for(Duration::from_secs(interval)).await;
    // Verrouille et vide le compteur.
    let mut compteur = state.rate_limit_counter.lock().await;
    compteur.clear();
}


/// Vérifie de manière active un serveur amont spécifique sans verrouillage.
///
/// # Arguments
///
/// * `state` - L'état partagé du proxy.
/// * `idx` - L'indice du serveur amont à vérifier.
/// * `chemin` - Le chemin de l'URL à vérifier sur le serveur amont.
///
/// # Returns
///
/// Retourne `Some(1)` si le serveur amont est accessible, sinon `None`.
///
/// # Example
///
/// ```
/// use std::sync::Arc;
/// use tokio::sync::Mutex;
/// use crate::ProxyState;
///
/// let state = Arc::new(ProxyState::new());
/// let idx = 0;
/// let chemin = String::from("/healthcheck");
/// let result = check_server(&state, idx, &chemin).await;
/// assert_eq!(result, Some(1));
/// ```

async fn check_server(state: &Arc<ProxyState>, idx: usize, chemin: &String) -> Option<usize> {
    // Prend l'IP du serveur amont.
    let ip_amont = &state.upstream_addresses[idx];
    // Tente de se connecter au serveur.
    let mut flux = connect_to_server(idx, &state).await.ok()?;
    // Construit la requête HTTP.
    let requête = http::Request::builder()
        .method(http::Method::GET)
        .uri(chemin)
        .header("Host", ip_amont)
        .body(Vec::new())
        .unwrap();
    // Envoie la requête.
    let _ = request::write_to_stream(&requête, &mut flux).await.ok()?;
    // Lit la réponse.
    let réponse = response::read_from_stream(&mut flux, &http::Method::GET).await.ok()?;
    // Si le statut est 200, le serveur est OK.
    if réponse.status().as_u16() != 200 {
        None
    } else {
        Some(1)
    }
}

/// Vérifie régulièrement l'état des serveurs amont.
///
/// # Arguments
///
/// * `state` - L'état partagé du proxy.
///
/// La fonction utilise l'état partagé pour vérifier périodiquement l'état de chaque serveur amont.
/// Si un serveur répond à la vérification de l'état, il est marqué comme disponible.
/// Si un serveur ne répond pas à la vérification de l'état, il est marqué comme indisponible.
///
/// # Example
///
/// ```
/// use std::sync::Arc;
/// use crate::ProxyState;
///
/// let state = Arc::new(ProxyState::new());
/// active_health_check(state).await;
/// ```
async fn active_health_check(state: Arc<ProxyState>) {
    let interval = state.active_health_check_interval as u64;
    let chemin = &state.active_health_check_path;
    loop {
        delay_for(Duration::from_secs(interval)).await;
        let mut status = state.upstream_status.write().await;
        for idx in 0..status.1.len() {
            // Si le serveur répond, on note qu'il est dispo.
            if check_server(&state, idx, chemin).await.is_some() {
                if !status.1[idx] {
                    status.0 += 1;
                    status.1[idx] = true;
                }
            } else {
                // Sinon, on note qu'il est en panne.
                if status.1[idx] {
                    status.0 -= 1;
                    status.1[idx] = false;
                }
            }
        }
    }
}

/// Choisit aléatoirement un serveur amont disponible.
///
/// # Arguments
///
/// * `state` - L'état partagé du proxy.
///
/// La fonction sélectionne aléatoirement un serveur parmi ceux qui sont marqués comme disponibles.
/// Si aucun serveur n'est disponible, elle renvoie `None`.
///
/// # Example
///
/// ```
/// use std::sync::Arc;
/// use crate::ProxyState;
///
/// let state = Arc::new(ProxyState::new());
/// let random_upstream = pick_random_alive_upstream(&state).await;
/// ```
async fn pick_random_alive_upstream(state: &Arc<ProxyState>) -> Option<usize> {
    let mut rng = rand::rngs::StdRng::from_entropy();
    let status = state.upstream_status.read().await;
    // Si y a pas de serveur dispo, renvoie rien.
    if status.0 == 0 {
        return None;
    }
    let mut idx;
    loop {
        idx = rng.gen_range(0, state.upstream_addresses.len());
        if status.1[idx] {
            return Some(idx);
        }
    }
}

/// Tente d'établir une connexion vers un serveur amont.
///
/// # Arguments
///
/// * `upstream_idx` - L'index du serveur amont dans la liste des adresses.
/// * `state` - L'état partagé du proxy.
///
/// La fonction essaie d'établir une connexion TCP avec le serveur amont spécifié par son index.
/// Si la connexion réussit, elle renvoie un `TcpStream` établi.
/// Sinon, elle renvoie une erreur contenant des informations sur l'échec de la connexion.
///
/// # Example
///
/// ```
/// use std::sync::Arc;
/// use crate::{ProxyState, connect_to_server};
/// use tokio::net::TcpStream;
///
/// let state = Arc::new(ProxyState::new());
/// let upstream_idx = 0; // Index du serveur amont à connecter
/// let result = connect_to_server(upstream_idx, &state).await;
/// match result {
///     Ok(stream) => println!("Connexion établie avec succès: {:?}", stream),
///     Err(err) => println!("Erreur lors de la connexion: {}", err),
/// }
/// ```
async fn connect_to_server(upstream_idx: usize, state: &Arc<ProxyState>) -> Result<TcpStream, std::io::Error> {
    let ip = &state.upstream_addresses[upstream_idx];
    match TcpStream::connect(ip).await {
        Ok(flux) => Ok(flux),
        Err(err) => {
            log::error!("Impossible de se connecter à l'amont {}: {}", ip, err);
            Err(err)
        }
    }
}
/// Essaye de se connecter à un serveur amont disponible. Renvoie une erreur si tous sont hors service.
///
/// # Arguments
///
/// * `state` - L'état actuel du proxy.
///
/// # Retourne
///
/// Une `TcpStream` si la connexion réussit, sinon une erreur d'entrée/sortie.
///
/// # Exemple
///
/// ```
/// use std::sync::Arc;
/// use tokio::net::TcpStream;
/// use std::io::{Error, ErrorKind};
/// use crate::ProxyState;
///
/// # #[tokio::main]
/// # async fn main() -> Result<(), Error> {
/// # let state = Arc::new(ProxyState);
/// match connect_to_upstream(state).await {
///     Ok(stream) => {
///         println!("Connecté au serveur amont !");
///         Ok(())
///     }
///     Err(err) => {
///         eprintln!("Erreur de connexion au serveur amont : {}", err);
///         Err(err)
///     }
/// }
/// # }
/// ```
async fn connect_to_upstream(state: Arc<ProxyState>) -> Result<TcpStream, std::io::Error> {
    loop {
        // Cherche un serveur amont disponible au hasard.
        if let Some(idx) = pick_random_alive_upstream(&state).await {
            match connect_to_server(idx, &state).await {
                Ok(stream) => return Ok(stream),
                Err(_) => {
                    // Si erreur, marque le serveur comme hors service.
                    let mut status = state.upstream_status.write().await;
                    status.0 -= 1;
                    status.1[idx] = false;
                }
            }
        } else {
            // Si tous les serveurs sont hors service, renvoie une erreur.
            return Err(std::io::Error::new(ErrorKind::Other, "Tous les serveurs amont sont hors service !"));
        }
    }
}


/// Envoie une réponse au client.
///
/// # Arguments
///
/// * `client_conn` - La connexion TCP avec le client.
/// * `réponse` - La réponse HTTP à envoyer.
///
/// Cette fonction envoie la réponse HTTP spécifiée au client via la connexion TCP.
/// Elle récupère également l'adresse IP du client pour le logging.
/// Si l'envoi de la réponse échoue, un avertissement est affiché dans les logs.
///
/// # Example
///
/// ```
/// use tokio::net::TcpStream;
/// use http::Response;
/// use crate::send_response;
///
/// let mut client_conn = TcpStream::connect("127.0.0.1:8080").await.unwrap();
/// let response = Response::builder().status(200).body(Vec::new()).unwrap();
/// send_response(&mut client_conn, &response).await;
/// ```
async fn send_response(client_conn: &mut TcpStream, réponse: &http::Response<Vec<u8>>) {
    // Récupère l'IP du client et log la réponse envoyée.
    let ip_client = client_conn.peer_addr().unwrap().ip().to_string();
    log::info!("{} <- {}", ip_client, response::format_response_line(&réponse));
    // Tente d'envoyer la réponse. Log si ça marche pas.
    if let Err(erreur) = response::write_to_stream(&réponse, client_conn).await {
        log::warn!("Impossible d'envoyer la réponse au client : {}", erreur);
    }
}




/// Traite chaque connexion client : reçoit une requête, la transmet à un serveur amont, renvoie la réponse.
///
/// # Arguments
///
/// * `client_conn` - La connexion TCP avec le client.
/// * `state` - L'état partagé de l'application.
///
/// Cette fonction gère chaque connexion entrante d'un client. Elle reçoit une requête HTTP du client,
/// la transmet à un serveur amont disponible, puis renvoie la réponse du serveur amont au client.
///
/// Si la connexion au serveur amont échoue, elle envoie une réponse d'erreur 502 (Bad Gateway) au client.
/// Si la lecture de la requête du client échoue ou si la requête est mal formée, elle envoie une réponse
/// d'erreur 400 (Bad Request) au client.
/// Si la lecture de la réponse du serveur amont échoue, elle envoie une réponse d'erreur 502 (Bad Gateway) au client.
///
/// # Example
///
/// ```
/// use tokio::net::TcpStream;
/// use std::sync::Arc;
/// use crate::{ProxyState, handle_connection};
///
/// let client_conn = TcpStream::connect("127.0.0.1:8080").await.unwrap();
/// let state = Arc::new(ProxyState::default());
/// handle_connection(client_conn, state).await;
/// ```
async fn handle_connection(mut client_conn: TcpStream, state: Arc<ProxyState>) {
    // Récupère l'IP du client pour le log.
    let ip_client = client_conn.peer_addr().unwrap().ip().to_string();
    log::info!("Connexion reçue de {}", ip_client);

    // Essaye de se connecter à un serveur amont dispo.
    let mut connexion_amont = match connect_to_upstream(state.clone()).await {
        Ok(flux) => flux,
        Err(_) => {
            // Si ça marche pas, envoie erreur 502 (Bad Gateway) au client.
            let réponse = response::make_http_error(http::StatusCode::BAD_GATEWAY);
            send_response(&mut client_conn, &réponse).await;
            return;
        }
    };
    let ip_amont = connexion_amont.peer_addr().unwrap().ip().to_string();

    loop {
        // Lis une requête du client.
        let mut requête = match request::read_from_stream(&mut client_conn).await {
            Ok(requête) => requête,
            Err(request::Error::IncompleteRequest(0)) => {
                // Si le client a fini d'envoyer des requêtes, arrête.
                log::debug!("Client a fini. On ferme la connexion.");
                return;
            },
            Err(erreur) => {
                // Si erreur de lecture ou requête mal formée, renvoie erreur au client et continue.
                log::debug!("Erreur requête : {:?}", erreur);
                let réponse = response::make_http_error(match erreur {
                    _ => http::StatusCode::BAD_REQUEST,
                });
                send_response(&mut client_conn, &réponse).await;
                continue;
            }
        };
        log::info!("{} -> {}: {}", ip_client, ip_amont, request::format_request_line(&requête));

        // Ajoute un en-tête pour que le serveur amont connaisse l'IP du client.
        request::extend_header_value(&mut requête, "x-forwarded-for", &ip_client);

        // Envoie la requête au serveur amont.
        if let Err(erreur) = request::write_to_stream(&requête, &mut connexion_amont).await {
            log::error!("Erreur envoi au serveur amont {}: {}", ip_amont, erreur);
            let réponse = response::make_http_error(http::StatusCode::BAD_GATEWAY);
            send_response(&mut client_conn, &réponse).await;
            return;
        }

        // Lit la réponse du serveur amont.
        let réponse = match response::read_from_stream(&mut connexion_amont, requête.method()).await {
            Ok(réponse) => réponse,
            Err(_) => {
                // Si problème à lire la réponse, renvoie erreur 502 au client.
                let réponse = response::make_http_error(http::StatusCode::BAD_GATEWAY);
                send_response(&mut client_conn, &réponse).await;
                return;
            }
        };
        // Renvoie la réponse du serveur amont au client.
        send_response(&mut client_conn, &réponse).await;
    }
}
