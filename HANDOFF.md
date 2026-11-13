# HANDOFF · atic-play — 2026-11-13

> **Actualización 2026-11-13:** la reescritura de `C:\dev\atic-play` ya se subió a `chrx3/atic-play` en la rama `wip/migracion-mac-reescritura` (historia independiente de `master`). El paso 1 de "Próximos pasos" ya está hecho.


> **ATENCIÓN, ANTES DE APAGAR EL WINDOWS:** el código nuevo de Atic Play **no está en GitHub**.
> Vive solo en `C:\dev\atic-play` (git inicializado, rama `master` **sin commits**, sin remoto).
> El clon del Mac `~/dev/atic-play` trae lo que hay en `chrx3/atic-play`, que es el código
> **viejo** (el de `C:\dev\ytm-player`). Hay que subir o copiar `C:\dev\atic-play` primero.

- Repo destino: `chrx3/atic-play` · Mac: `~/dev/atic-play`
- Remoto hoy: `master` = `legacy` = `a60634f` (2026-10-20, «Prepare the repository for publishing»), el código viejo (`ytm-player`). También ramas locales viejas `downloads-ui`, `robustness-ux` en `C:\dev\ytm-player`.
- Trabajo nuevo: `C:\dev\atic-play`, sin commit. No hay rama `wip`.

## En qué estaba
Reescritura desde cero (chat «atic-play», 2026-11-12) de un reproductor de YouTube Music en Rust + GPUI + gpui-m3, hecha con subagentes OpenCode. Workspace de 5 crates:
- `crates/ytmusic` (`play-ytmusic`): cliente propio de YouTube Music (búsqueda, inicio, álbum, artista, playlist, cola, URL de audio; y con cuenta: Me gusta, listas, biblioteca, historial, inicio personalizado, dar me gusta, agregar/crear lista).
- `crates/audio` (`play-audio`): descarga por rangos HTTP, decodifica fMP4 (symphonia), suena (cpal).
- `crates/lyrics` (`play-lyrics`): letras sincronizadas de LRCLIB.
- `crates/account` (`play-account`): sesión de Google en el llavero + binario `atic-play-login` (ventana de login con tao/wry).
- `crates/app` (`atic-play`): la app (riel, Biblioteca, Reproduciendo, cola, letra, atajos, panel de medios de Windows).

## Estado
- **Hecho (Windows):** compila sin warnings, pasan los tests de los 5 crates; la app abrió con la sesión del usuario y cargó inicio personalizado y Biblioteca. Arreglado el parpadeo de carátulas (caché 192 MB, no bota lo dibujado en los últimos 10 s, no reintenta 404).
- **Sin probar:** dar me gusta / agregar a lista / crear lista contra la cuenta real; teclas multimedia físicas; sesión vencida; hover y foco con Tab; espera ante 429 de LRCLIB.
- **Bugs conocidos:** la sesión de medios del sistema dice «Playing» sin nada cargado; la «galleta» del fondo salta al pausar; elipsis «…» rara en algunos anchos; letras: si ningún resultado dura parecido (±5 s) usa el más cercano (puede ser otra versión).
- **Pregunta abierta (usuario dijo «muy verde»):** el color dinámico toma el tono de la portada. Opciones: (1) teñir solo reproductor y hoja «Reproduciendo», resto neutro; (2) bajar intensidad global. Falta elegir.
- **Auditorías ya hechas:** `/impeccable` dio 9/20 (6 P1, 11 P2, 3 P3) y hubo otra con `/make-interfaces-feel-better`; sus arreglos se repartieron y aplicaron (teclado/foco, SMTC, scroll, barra a 980/800 px, caché, tema según el sistema, preferencias, «Deshacer» en Vaciar). Queda sin resolver: lectores de pantalla (GPUI 0.2.2 no expone accesibilidad) y «Artistas» de la Biblioteca salía en 0 (luego carga 30; confirmar). El informe quedó en una carpeta temporal de Windows, no migrada.

## Archivos clave
- `AGENTS.md` (reglas: código propio, `-j 4`, no commits de subagentes), `Cargo.toml` (con `[patch]` temporal)
- `crates/app/src/{main,model,player,queue,library,account,source,media_controls,settings,covers,shortcuts}.rs`
- `crates/account/src/{lib.rs,bin/atic-play-login.rs}`

## Próximos pasos
1. **En Windows, antes de irse:** en `C:\dev\atic-play` agregar remoto `chrx3/atic-play`, hacer el primer commit (sin `target/`, ya ignorado) y subirlo a una rama nueva, p. ej. `rewrite` o `wip/migracion-mac` (más seguro que el force push a `master`). Alternativa: copiar la carpeta sin `target/`.
2. En el Mac: `git fetch` y `git checkout` esa rama en `~/dev/atic-play`.
3. Cambiar el `[patch]` de `Cargo.toml`: hoy apunta a `C:/Users/Christian/OneDrive - .../gpui-m3`. En Mac usar `path = "../gpui-m3"` con `~/dev/gpui-m3` en la rama `wip/migracion-mac`, o mejor publicar gpui-m3 y quitar el `[patch]` (ver `gpui-m3.md`).
4. Compilar en Mac (`cargo build -j 4`) y portar lo de Windows (ver abajo).
5. Elegir opción de color, correr las auditorías y probar lo «sin probar».
6. Cuando esté listo: reemplazar `master` con la reescritura (el código viejo queda en `legacy`). Preguntar antes de cada push.

## Decisiones y contexto
- **Código limpio (obligatorio, en `AGENTS.md`):** licencia MIT OR Apache-2.0, todo desde cero. No leer ni copiar código de otros clientes (Echo-Music, Metrolist, InnerTune, ViMusic, NewPipe, rustypipe, yt-dlp, ytmusicapi, YouTube.js, Piped…), **ni del repo viejo `ytm-player`** (que es lo que hoy trae el clon del Mac en `master`) ni de `C:\dev\research`.
- Cuenta: usa cookies de una sesión web de Google con la API interna de YouTube Music (no oficial; riesgo bajo de que Google la marque). Si Google bloquea el login en ventana integrada, no saltarse la protección; respaldo: pegar cookies a mano. Nunca registrar cookies en logs.
- El audio sigue por la vía anónima aun con sesión.
- Subagentes: se usó la cuota OpenCode Go (`deepseek-v4.1-flash`, `minimax-m3.1-flash`); el usuario aceptó usar los modelos gratis cuando convenga.
- Pruebas que suenan: siempre con volumen 0 y verificar por estado/posición (el usuario estaba en reunión).
- Atic (la pill) ya reconoce a Atic Play por nombre en Windows (sesión SMTC `atic-play.exe`).

## Notas para macOS
- **Windows-only:** `crates/app/src/media_controls.rs` (SystemMediaTransportControls/WinRT), `settings.rs` (barra de título oscura/ícono vía `windows`/`windows-sys`); en Mac hay ramas `cfg(not(windows))` sin equivalente → portar a `MPNowPlayingInfoCenter`/`MPRemoteCommandCenter` para que Atic Mac y las teclas multimedia lo vean.
- Login: `wry` usa WebView2 en Windows y WKWebView en Mac; probar si Google permite el login ahí.
- Llavero: `keyring` 4 con `default-features = false, features = ["v1"]`; por confirmar que en Mac use el Keychain (puede faltar el backend de Apple).
- Logs/preferencias/caché: revisar rutas (`settings.json` en el directorio de datos del usuario) en `~/Library/...`.
- Atic Mac lee medios con MediaRemote desde `osascript`; verificar que detecte a Atic Play una vez publicado el Now Playing.

## Cómo integrar
No hay base ni wip comparables: es un historial nuevo. Primer commit en una rama nueva → probar en Mac → `git push origin <rama>:master --force-with-lease` solo cuando el usuario lo apruebe (el viejo queda en `legacy`).
