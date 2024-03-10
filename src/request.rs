use std::cmp::min;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
// Définis les tailles max pour les en-têtes et le corps des requêtes.
const MAX_HEADERS_SIZE: usize = 8000;
const MAX_BODY_SIZE: usize = 10000000;
const MAX_NUM_HEADERS: usize = 32;

// Différents types d'erreurs qu'on peut rencontrer.
#[derive(Debug)]
pub enum Error {
    // Le client a coupé la connexion trop tôt.
    IncompleteRequest(usize),
    // La requête HTTP envoyée par le client n'est pas valide.
    MalformedRequest(httparse::Error),
    // L'en-tête Content-Length n'est pas un nombre valide.
    InvalidContentLength,
    // La taille du corps de la requête ne correspond pas à celle annoncée par Content-Length.
    ContentLengthMismatch,
    // Le corps de la requête dépasse la taille maximale autorisée.
    RequestBodyTooLarge,
    // Une erreur de lecture/écriture dans le TcpStream.
    ConnectionError(std::io::Error),
}

/// Récupère la valeur de l'en-tête Content-Length d'une requête.
///
/// # Arguments
///
/// * `request` - La requête HTTP dont on souhaite récupérer l'en-tête Content-Length.
///
/// # Returns
///
/// Retourne `Ok(Some(length))` si l'en-tête Content-Length existe dans la requête et que sa valeur
/// peut être convertie en un nombre entier usize.
/// Retourne `Ok(None)` si l'en-tête Content-Length n'existe pas dans la requête.
/// Retourne une erreur `Error::InvalidContentLength` si la valeur de l'en-tête Content-Length n'est
/// pas valide ou ne peut pas être convertie en un nombre entier usize.
///
/// # Example
///
/// ```
/// use http::Request;
/// use crate::get_content_length;
///
/// let request_with_content_length = Request::builder()
///     .header("Content-Length", "100")
///     .body(Vec::new())
///     .unwrap();
///
/// let content_length = get_content_length(&request_with_content_length).unwrap();
/// assert_eq!(content_length, Some(100));
/// ```

fn get_content_length(request: &http::Request<Vec<u8>>) -> Result<Option<usize>, Error> {
    if let Some(header_value) = request.headers().get("content-length") {
        // Convertit la valeur en nombre si possible, sinon renvoie une erreur.
        Ok(Some(
            header_value
                .to_str()
                .or(Err(Error::InvalidContentLength))?
                .parse::<usize>()
                .or(Err(Error::InvalidContentLength))?,
        ))
    } else {
        // Si l'en-tête Content-Length n'existe pas, retourne None.
        Ok(None)
    }
}

/// Ajoute ou met à jour la valeur d'un en-tête dans une requête.
///
/// # Arguments
///
/// * `request` - La référence mutable de la requête HTTP à laquelle on souhaite ajouter ou mettre à jour l'en-tête.
/// * `name` - Le nom de l'en-tête à ajouter ou mettre à jour.
/// * `extend_value` - La valeur à ajouter ou concaténer à l'en-tête existant.
///
/// # Example
///
/// ```
/// use http::Request;
/// use crate::extend_header_value;
///
/// let mut request = Request::builder()
///     .body(Vec::new())
///     .unwrap();
///
/// extend_header_value(&mut request, "Content-Type", "application/json");
/// ```

pub fn extend_header_value(
    request: &mut http::Request<Vec<u8>>,
    name: &'static str,
    extend_value: &str,
) {
    // Crée une nouvelle valeur pour l'en-tête en ajoutant la valeur fournie.
    let new_value = match request.headers().get(name) {
        Some(existing_value) => {
            [existing_value.as_bytes(), b", ", extend_value.as_bytes()].concat()
        }
        None => extend_value.as_bytes().to_owned(),
    };
    // Met à jour l'en-tête avec la nouvelle valeur.
    request
        .headers_mut()
        .insert(name, http::HeaderValue::from_bytes(&new_value).unwrap());
}
/// Essaye de transformer les données du buffer en une requête HTTP.
///
/// Ça peut donner :
///
/// * `Ok(Some(http::Request))` si on a une requête complète et valide.
/// * `Ok(None)` si la requête n'est pas finie mais semble valide jusqu'ici.
/// * `Err(Error)` si ce qu'on a ne ressemble pas du tout à une requête valide.
///
/// # Arguments
///
/// * `buffer` - Le buffer contenant les données à analyser pour former la requête HTTP.
///
/// # Returns
///
/// Retourne un `Result` :
///
/// * `Ok(Some(http::Request<Vec<u8>>>)` - Une requête HTTP valide et complète avec sa taille.
/// * `Ok(None)` - La requête n'est pas finie mais semble valide jusqu'ici.
/// * `Err(Error)` - Une erreur si ce qu'on a ne ressemble pas du tout à une requête valide.
///
/// # Example
///
/// ```
/// use crate::parse_request;
/// use crate::Error;
///
/// let buffer = b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n";
/// match parse_request(buffer) {
///     Ok(Some(request)) => println!("Requête HTTP valide : {:?}", request),
///     Ok(None) => println!("Requête HTTP incomplète mais valide jusqu'ici."),
///     Err(Error::MalformedRequest(_)) => println!("Erreur : Requête mal formée."),
///     _ => println!("Autre erreur."),
/// }
/// ```

fn parse_request(buffer: &[u8]) -> Result<Option<(http::Request<Vec<u8>>, usize)>, Error> {
    // Prépare un endroit pour stocker les en-têtes.
    let mut headers = [httparse::EMPTY_HEADER; MAX_NUM_HEADERS];
    let mut req = httparse::Request::new(&mut headers);
    // Essaye de lire la requête dans le buffer.
    let res = req.parse(buffer).or_else(|err| Err(Error::MalformedRequest(err)))?;

    // Si on a tout ce qu'il faut pour une requête complète :
    if let httparse::Status::Complete(len) = res {
        // Construit la requête HTTP avec les infos qu'on a récoltées.
        let mut request_builder = http::Request::builder()
            .method(req.method.unwrap())
            .uri(req.path.unwrap())
            .version(http::Version::HTTP_11);
        for header in req.headers {
            request_builder = request_builder.header(header.name, header.value);
        }
        let request = request_builder.body(Vec::new()).unwrap();
        // Retourne la requête et sa taille.
        Ok(Some((request, len)))
    } else {
        // Si on n'a pas encore tout, ou que c'est pas valide, on dit qu'on n'a rien.
        Ok(None)
    }
}
/// Lit les en-têtes d'une requête HTTP depuis le stream donné.
///
/// Cette fonction attend jusqu'à ce qu'un ensemble complet d'en-têtes soit envoyé.
/// Elle ne lit que la ligne de requête et les en-têtes; pour lire le corps de la requête
/// (comme pour une requête POST), il faut appeler ensuite la fonction read_body.
///
/// Renvoie `Ok(http::Request)` si une requête valide est reçue, ou une erreur sinon.
///
/// # Arguments
///
/// * `stream` - Le flux TcpStream à partir duquel lire les en-têtes de la requête.
///
/// # Returns
///
/// Retourne un `Result` :
///
/// * `Ok(http::Request<Vec<u8>>)` - Une requête HTTP valide si reçue avec succès.
/// * `Err(Error)` - Une erreur si la requête est incomplète ou mal formée, ou s'il y a une erreur de connexion.
///
/// # Example
///
/// ```no_run
/// use tokio::net::TcpStream;
/// use crate::{read_headers, Error};
///
/// #[tokio::main]
/// async fn main() -> Result<(), Error> {
///     let mut stream = TcpStream::connect("127.0.0.1:8080").await?;
///     match read_headers(&mut stream).await {
///         Ok(request) => println!("Requête HTTP reçue : {:?}", request),
///         Err(err) => println!("Erreur lors de la lecture des en-têtes : {:?}", err),
///     }
///     Ok(())
/// }
/// ```
async fn read_headers(stream: &mut TcpStream) -> Result<http::Request<Vec<u8>>, Error> {
    let mut request_buffer = [0_u8; MAX_HEADERS_SIZE];
    let mut bytes_read = 0;
    loop {
        let new_bytes = stream
            .read(&mut request_buffer[bytes_read..]).await
            .or_else(|err| Err(Error::ConnectionError(err)))?;
        if new_bytes == 0 {
            return Err(Error::IncompleteRequest(bytes_read));
        }
        bytes_read += new_bytes;

        if let Some((mut request, headers_len)) = parse_request(&request_buffer[..bytes_read])? {
            request.body_mut().extend_from_slice(&request_buffer[headers_len..bytes_read]);
            return Ok(request);
        }
    }
}



// Lit le corps d'une requête depuis le stream.
// Le corps est envoyé par le client uniquement si l'en-tête Content-Length est présent.
// Cette fonction lit alors le nombre spécifié d'octets depuis le stream.
// Renvoie Ok(()) si tout se passe bien, sinon une erreur.
async fn read_body(
    stream: &mut TcpStream,
    request: &mut http::Request<Vec<u8>>,
    content_length: usize,
) -> Result<(), Error> {
    while request.body().len() < content_length {
        let mut buffer = vec![0_u8; min(512, content_length - request.body().len())];
        let bytes_read = match stream.read(&mut buffer).await {
            Ok(bytes_read) => bytes_read,
            Err(err) => return Err(Error::ConnectionError(err)),
        };

        if bytes_read == 0 || request.body().len() + bytes_read > content_length {
            return Err(Error::ContentLengthMismatch);
        }

        request.body_mut().extend_from_slice(&buffer[..bytes_read]);
    }
    Ok(())
}


// Lit une requête HTTP depuis un flux.
// Renvoie une erreur si le client ferme la connexion trop tôt ou envoie une requête invalide.
pub async fn read_from_stream(stream: &mut TcpStream) -> Result<http::Request<Vec<u8>>, Error> {
    // Lit d'abord les entêtes.
    let mut requete = read_headers(stream).await?;
    // Si y a un Content-Length (comme pour les POST), lit le corps.
    if let Some(taille_contenu) = get_content_length(&requete)? {
        if taille_contenu > MAX_BODY_SIZE {
            // Trop gros, pas bon.
            return Err(Error::RequestBodyTooLarge);
        } else {
            // Lit le corps de la requête.
            read_body(stream, &mut requete, taille_contenu).await?;
        }
    }
    Ok(requete)
}

// Transforme une requête en octets et les envoie sur le flux.
pub async fn write_to_stream(
    requete: &http::Request<Vec<u8>>,
    stream: &mut TcpStream,
) -> Result<(), std::io::Error> {
    // Envoie la ligne de requête.
    stream.write(&format_request_line(requete).into_bytes()).await?;
    stream.write(b"\r\n").await?; // Retour à la ligne.
    // Envoie chaque entête.
    for (nom_entete, valeur_entete) in requete.headers() {
        stream.write(format!("{}: ", nom_entete).as_bytes()).await?;
        stream.write(valeur_entete.as_bytes()).await?;
        stream.write(b"\r\n").await?; // Retour à la ligne.
    }
    stream.write(b"\r\n").await?;
    // Si y a un corps, l'envoie.
    if !requete.body().is_empty() {
        stream.write(requete.body()).await?;
    }
    Ok(())
}

// Formate la ligne de requête pour l'envoyer.
pub fn format_request_line(requete: &http::Request<Vec<u8>>) -> String {
    format!("{} {} {:?}", requete.method(), requete.uri(), requete.version())
}
