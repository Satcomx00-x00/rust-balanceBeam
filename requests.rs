// Importe la fonction `min` pour la comparaison de valeurs.
use std::cmp::min;


// Importe les extensions asynchrones pour la lecture et l'écriture de flux.
use tokio::io::{AsyncReadExt, AsyncWriteExt};
// Importe la version asynchrone de TcpStream pour les opérations de réseau TCP.
use tokio::net::TcpStream; 

// Définit la taille maximale des en-têtes HTTP.
const MAX_HEADERS_SIZE: usize = 8000;
// Définit la taille maximale du corps d'une requête/réponse HTTP.
const MAX_BODY_SIZE: usize = 10000000;
// Définit le nombre maximal d'en-têtes HTTP autorisés.
const MAX_NUM_HEADERS: usize = 32;

// L'objectif est d'effectuer des opérations réseau tcp , en particulier la lecture et l'écriture de données.

pub enum Error {
    // Erreur si le client ferme la connexion avant d'envoyer une requête complète.
    IncompleteRequest(usize),
    
    // Erreur si le client envoie une requête HTTP mal formée.
    MalformedRequest(httparse::Error),

    // Erreur si l'en-tête Content-Length n'est pas une valeur numérique valide.
    InvalidContentLength,

    // Erreur si la longueur du contenu ne correspond pas à celle indiquée par l'en-tête Content-Length.
    ContentLengthMismatch,

    // Erreur si le corps de la requête est plus grand que MAX_BODY_SIZE.
    RequestBodyTooLarge,

    // Erreur lors de la lecture/écriture d'un TcpStream due à un problème d'I/O.
    ConnectionError(std::io::Error),

    // Erreur lorsqu'aucun serveur amont n'est disponible pour traiter la requête.
    UpstreamServerUnavailable,
}
// Cette fonction tente de récupérer la longueur du contenu (Content-Length) d'une requête HTTP.
fn get_content_length(request: &http::Request<Vec<u8>>) -> Result<Option<usize>, Error> {
    // Recherche de l'en-tête "content-length" dans la requête.
    if let Some(header_value) = request.headers().get("content-length") {
        // Si l'en-tête existe, essaie de le parser en tant que `usize`.
        // Renvoie une erreur `Error::InvalidContentLength` si le parsing échoue.
        Ok(Some(
            header_value
                .to_str() // Convertit la valeur de l'en-tête en chaîne de caractères.
                .or(Err(Error::InvalidContentLength))? // Gère l'erreur si la conversion en chaîne échoue.
                .parse::<usize>() // Parse la chaîne en `usize`.
                .or(Err(Error::InvalidContentLength))? // Gère l'erreur si le parsing en `usize` échoue.
        ))
    } else {
        // Si l'en-tête "content-length" n'existe pas, renvoie `None`.
        Ok(None)
    }
}
