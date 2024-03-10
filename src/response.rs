//! Bibliothèque asynchrone pour le traitement des réponses HTTP via TCP.
//!
//! Ce module fournit les outils nécessaires pour lire et écrire des réponses HTTP de manière asynchrone
//! sur des connexions TCP. Il gère les tailles maximales des en-têtes et du corps des réponses pour éviter
//! la saturation des données, et catégorise différentes erreurs qui peuvent survenir lors du traitement.
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;


/// je definis les constantes pour les tailles maximales des en-têtes et du corps de la réponse, ainsi que le nombre maximum d'en-têtes , afin déviter la saturation de données.
const MAX_HEADERS_SIZE: usize = 8000;
const MAX_BODY_SIZE: usize = 10000000;
const MAX_NUM_HEADERS: usize = 32;

/// je catégorise les différentes erreurs qui peuvent survenir lors du traitement des réponses HTTP.
#[derive(Debug)]
/// les différences erreurs qui peuvent survenir lors du traitement des réponses HTTP.
pub enum Error {
    /// Le client a interrompu la connexion avant d'envoyer une requête complète.
    IncompleteResponse,
    /// Le client a envoyé une requête HTTP invalide. Pour plus de détails, voir `httparse::Error`.
    MalformedResponse(httparse::Error),
    /// L'en-tête Content-Length est présent mais ne contient pas une valeur numérique valide.
    InvalidContentLength,
    /// L'en-tête Content-Length ne correspond pas à la taille du corps de la requête envoyée.
    ContentLengthMismatch,
    /// La taille du corps de la requête dépasse MAX_BODY_SIZE.
    ResponseBodyTooLarge,
    /// Une erreur d'entrée/sortie s'est produite lors de la lecture ou de l'écriture sur un TcpStream.
    ConnectionError(std::io::Error),
}

/// cette fonction permet d'extraire la valeur de l'en-tête Content-Length d'une réponse HTTP.
fn get_content_length(response: &http::Response<Vec<u8>>) -> Result<Option<usize>, Error> {
    if let Some(header_value) = response.headers().get("content-length") {
        header_value.to_str()
            .or(Err(Error::InvalidContentLength))
            .and_then(|s| s.parse().map(Some).map_err(|_| Error::InvalidContentLength))
    } else {
        Ok(None)
    }
}

/// Analyse une réponse HTTP à partir d'un tampon d'octets.
///
/// # Arguments
///
/// * `buffer` - Tampon d'octets contenant la réponse HTTP à analyser.
///
/// # Retourne
///
/// Un résultat contenant soit une option de `(http::Response<Vec<u8>>, usize)` en cas de succès, soit une erreur.
/// L'option contient `None` si la réponse est partielle, ou les données de la réponse et sa longueur si complète.
///
/// # Erreurs
///
/// Retourne une erreur si la réponse ne peut pas être analysée correctement.
fn parse_response(buffer: &[u8]) -> Result<Option<(http::Response<Vec<u8>>, usize)>, Error> {
    // Initialisation des entêtes avec une taille maximale prédéfinie.
    let mut headers = [httparse::EMPTY_HEADER; MAX_NUM_HEADERS];
    
    // Création d'un nouvel objet `Response` pour stocker la réponse analysée.
    let mut resp = httparse::Response::new(&mut headers);
    
    // Analyse du tampon d'octets pour remplir l'objet `resp`.
    match resp.parse(buffer) {
        // Si l'analyse est complète :
        Ok(httparse::Status::Complete(len)) => {
            // Construction d'une réponse HTTP avec les données analysées.
            let mut response_builder = http::Response::builder()
                // Ajout du statut HTTP à partir de `resp`.
                .status(resp.code.ok_or(Error::MalformedResponse(httparse::Error::TooManyHeaders))?)
                // Définition de la version HTTP sur 1.1.
                .version(http::Version::HTTP_11);
                
            // Ajout des entêtes HTTP à la réponse.
            for header in resp.headers.iter() {
                response_builder = response_builder.header(header.name, header.value);
            }
            
            // Création du corps de la réponse et retourne la réponse construite avec sa longueur.
            response_builder.body(Vec::new()).map(|body| Some((body, len)))
                .map_err(|_| Error::MalformedResponse(httparse::Error::TooManyHeaders))
        },
        // Si la réponse est partielle, retourne `None`.
        Ok(httparse::Status::Partial) => Ok(None),
        // En cas d'erreur lors de l'analyse, retourne l'erreur.
        Err(e) => Err(Error::MalformedResponse(e)),
    }
}

/// Lit les en-têtes d'une réponse HTTP à partir d'un flux TCP.
///
/// Cette fonction lit de manière asynchrone les données d'un `TcpStream` jusqu'à ce que tous les en-têtes HTTP soient reçus,
/// puis elle utilise `parse_response` pour analyser ces en-têtes et construire une réponse HTTP.
///
/// # Arguments
///
/// * `stream` - Le flux TCP (`TcpStream`) à partir duquel lire les données.
///
/// # Retourne
///
/// Un résultat contenant une `http::Response<Vec<u8>>` en cas de succès, ou une erreur en cas d'échec.
///
/// # Erreurs
///
/// Retourne une erreur si la connexion est interrompue avant que tous les en-têtes puissent être lus, ou si la réponse
/// ne peut pas être analysée correctement.
async fn read_headers(stream: &mut TcpStream) -> Result<http::Response<Vec<u8>>, Error> {
    // Crée un tampon pour stocker la réponse lue depuis le stream.
    let mut response_buffer = [0_u8; MAX_HEADERS_SIZE];
    // Suit le nombre d'octets lus.
    let mut bytes_read = 0;
    
    loop {
        // Lit les données depuis le stream dans le tampon.
        let new_bytes = stream
            .read(&mut response_buffer[bytes_read..]).await
            .map_err(Error::ConnectionError)?;
        // Vérifie si la connexion a été fermée.
        if new_bytes == 0 {
            return Err(Error::IncompleteResponse);
        }
        // Met à jour le compte des octets lus.
        bytes_read += new_bytes;

        // Tente d'analyser la réponse HTTP avec les données actuelles.
        if let Some((mut response, headers_len)) = parse_response(&response_buffer[..bytes_read])? {
            // Si la réponse est complète, ajoute le corps de la réponse et retourne.
            response.body_mut().extend_from_slice(&response_buffer[headers_len..bytes_read]);
            return Ok(response);
        }
    }
}

/// Lit le corps de la réponse HTTP d'un flux TCP et le colle à la réponse.
///
/// Fait ça en se basant sur l'en-tête Content-Length pour savoir combien faut lire.
/// Si Content-Length n'est pas là, lit jusqu'à ce qu'il n'y ait plus rien à lire.
///
/// # Arguments
///
/// * `stream` - D'où lire les données.
/// * `response` - La réponse à bourrer avec les données lues.
///
/// # Retourne
///
/// Rien si tout va bien, une erreur si ça coince quelque part.
async fn read_body(stream: &mut TcpStream, response: &mut http::Response<Vec<u8>>) -> Result<(), Error> {
    // Checke si on a un Content-Length.
    let content_length = get_content_length(response)?;

    // Lit le stream tant qu'on n'a pas tout le contenu.
    while content_length.is_none() || response.body().len() < content_length.unwrap() {
        // Buffer temporaire pour lire les données.
        let mut buffer = [0_u8; 512];
        // Combien on vient de lire.
        let bytes_read = stream
            .read(&mut buffer).await
            .map_err(Error::ConnectionError)?;
        // Si y a plus rien à lire, c'est soit fini, soit y a un souci.
        if bytes_read == 0 {
            if content_length.is_none() {
                break; // Fini de lire, pas de Content-Length.
            } else {
                return Err(Error::ContentLengthMismatch); // Pas bon, la taille colle pas.
            }
        }

        // Si on dépasse la taille annoncée, pas bon non plus.
        if let Some(length) = content_length {
            if response.body().len() + bytes_read > length {
                return Err(Error::ContentLengthMismatch); // Oups, trop lu.
            }
        }

        // Ajoute ce qu'on vient de lire au corps de la réponse.
        response.body_mut().extend_from_slice(&buffer[..bytes_read]);
    }
    Ok(())
}

/// Lit une réponse HTTP d'un flux TCP.
///
/// Si la requête n'est pas une méthode HEAD et que la réponse doit avoir un corps,
/// lit ce corps depuis le flux.
///
/// # Arguments
///
/// * `stream` - Le flux pour lire la réponse.
/// * `request_method` - La méthode HTTP de la requête pour savoir si faut lire le corps.
///
/// # Retourne
///
/// Une réponse HTTP complète ou une erreur.
pub async fn read_from_stream(stream: &mut TcpStream, request_method: &http::Method) -> Result<http::Response<Vec<u8>>, Error> {
    // Lit d'abord les entêtes.
    let mut response = read_headers(stream).await?;
    // Vérifie si faut un corps. Si oui, le lit.
    if !(request_method == &http::Method::HEAD || response.status().is_informational() || response.status() == http::StatusCode::NO_CONTENT || response.status() == http::StatusCode::NOT_MODIFIED) {
        read_body(stream, &mut response).await?;
    }
    Ok(response)
}

/// Écrit une réponse HTTP dans un flux TCP.
///
/// Convertit la réponse en bytes et l'envoie.
///
/// # Arguments
///
/// * `response` - La réponse à envoyer.
/// * `stream` - Où écrire la réponse.
///
/// # Retourne
///
/// Ok si tout s'est bien passé, une erreur d'IO sinon.
pub async fn write_to_stream(response: &http::Response<Vec<u8>>, stream: &mut TcpStream) -> Result<(), std::io::Error> {
    // Écrit la ligne de statut.
    stream.write_all(&format_response_line(response).into_bytes()).await?;
    stream.write_all(b"\r\n").await?;
    // Puis les entêtes.
    for (name, value) in response.headers() {
        stream.write_all(format!("{}: {:?}\r\n", name, value.to_str().unwrap_or("")).as_bytes()).await?;
    }
    // Un saut de ligne pour finir les entêtes.
    stream.write_all(b"\r\n").await?;
    // Si y a un corps, l'envoie aussi.
    if !response.body().is_empty() {
        stream.write_all(response.body()).await?;
    }
    Ok(())
}

/// Prend une réponse HTTP et prépare la première ligne pour l'envoyer.
///
/// Met ensemble la version HTTP, le code de statut, et la raison de ce statut.
/// Ajoute aussi un retour chariot et une nouvelle ligne à la fin.
///
/// # Arguments
///
/// * `response` - La réponse dont on tire les infos.
///
/// # Retourne
///
/// La ligne de statut formatée prête à être envoyée.
pub fn format_response_line(response: &http::Response<Vec<u8>>) -> String {
    format!("{:?} {} {:?}\r\n", response.version(), response.status(), response.status().canonical_reason().unwrap_or(""))

}

/// Crée une réponse HTTP avec une erreur dedans.
///
/// Utilise le code d'erreur pour créer une réponse HTTP complète avec ce code et la raison standard.
/// Met "text/plain" comme type de contenu et colle tout dans le corps.
///
/// # Arguments
///
/// * `status` - Le code d'erreur à utiliser.
///
/// # Retourne
///
/// La réponse HTTP avec l'erreur dedans.
pub fn make_http_error(status: http::StatusCode) -> http::Response<Vec<u8>> {
    // Formate la réponse avec l'erreur.
    let body = format!("HTTP/1.1 {} {:?}\r\n\r\n", status, status.canonical_reason().unwrap_or(""));
    // Construit la réponse avec le statut, un type de contenu, et le corps.
    http::Response::builder()
        .status(status)
        .header("Content-Type", "text/plain")
        .body(body.into_bytes())
        .unwrap()
}
