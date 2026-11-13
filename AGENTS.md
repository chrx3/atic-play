# Trabajo con agentes en Atic Play

Atic Play es un reproductor de música de escritorio en Rust puro, con
[GPUI](https://crates.io/crates/gpui) y [gpui-m3](https://github.com/chrx3/gpui-m3)
(Material 3 Expressive). Objetivo: rápido, cómodo y fácil.

## Mapa

| Crate | Qué hace |
| --- | --- |
| `crates/ytmusic` (`play-ytmusic`) | Cliente propio de YouTube Music: búsqueda, inicio, álbum, artista, playlist, cola, URL de audio |
| `crates/audio` (`play-audio`) | Motor de audio: descarga por rangos HTTP, decodifica (symphonia) y suena (cpal) |
| `crates/lyrics` (`play-lyrics`) | Letras sincronizadas desde LRCLIB y parser LRC |
| `crates/app` (`atic-play`) | La app GPUI |

La API pública de cada `lib.rs` es el contrato entre crates: no cambies nombres
ni campos sin avisar en tu informe final.

## Código limpio y propio (obligatorio)

- Licencia **MIT OR Apache-2.0**. Todo el código se escribe desde cero.
- **No leas, copies ni traduzcas** código de otros reproductores o clientes de
  YouTube / YouTube Music (Echo-Music, Metrolist, InnerTune, ViMusic, NewPipe,
  rustypipe, yt-dlp, ytmusicapi, YouTube.js, Piped, etc.), ni de la carpeta
  `C:\dev\ytm-player` ni de `C:\dev\research`. No los agregues como dependencia.
- Sí puedes: hacer peticiones reales a los endpoints para ver qué responden,
  leer la documentación de las librerías que usas (ureq, symphonia, cpal,
  gpui, gpui-m3) y el código de gpui-m3, que es nuestro.

## Compilar sin tumbar el PC

- Target compartido: `C:\dev\atic-play\target` (el normal). Cargo bloquea el
  directorio: si otro agente compila, espera; no uses otro target dir.
- Usa `-j 4`. Nunca lances dos compilaciones a la vez.
- Solo el agente de la app compila `atic-play` (GPUI tarda minutos y usa mucha
  memoria). Los demás: `cargo test -j 4 -p <su-crate>`.
- No corras `cargo update` a ciegas ni `cargo clean`.

## Estilo

- Comentarios y mensajes en inglés en el código; textos de la UI en español
  de Chile (tuteo).
- Archivos en UTF-8.
- Pruebas: unitarias con JSON/LRC de ejemplo guardados en `tests/fixtures`;
  las que tocan la red van con `#[ignore]` y se corren a mano.
- No hagas commits ni push: el orquestador los hace.
