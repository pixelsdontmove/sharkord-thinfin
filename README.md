<div align="center"> <h1>Thinfin</h1> <p><strong>Desktop client for https://github.com/sharkord/sharkord.</strong></p> </div>

## Credits

Sharkord is made by [Diogo Martino](https://github.com/sharkord/sharkord). This desktop client is just a wrapper. It is in no way or form endorsed by the Sharkord creator or supported by him.

## What is Thinfin?

> [!NOTE]
> Do not use this. I have to make it clear, I only made this for my friends and me. If you decide to use it, it will be at your own risk. There is a severe lack of compatibility and features.

Consider the Thinfin client/wrapper as designed by a fair mix of AI and human hallucinations forced into something that works on Windows. It's a lightweight [Tauri 2](https://tauri.app) shell (Rust + the system WebView2 runtime) - no Electron, no bundled browser, a few MB of binary.

## What does it do?

- Loads your hosted sharkord server in a native window with a custom title bar
- First run asks for your server URL once and remembers it (slots S1–S3)
- Global hotkeys for mute/deafen with option to toggle or hold.
- System audio capture for screen share (WASAPI on Windows) — the speaker button in the top bar toggles it, and it excludes Thinfin's own audio so you don't hear yourself echoed back

## Wanna Try It Out?

Windows 10/11 only for now. Run the exe and enter your server URL on first launch — that's the whole setup. Needs the [WebView2 runtime](https://developer.microsoft.com/en-us/microsoft-edge/webview2/), which is preinstalled on Windows 11 and most up-to-date Windows 10 machines.

## Why?

Because Qwen3.8 27B on a R9700 Pro is the way.