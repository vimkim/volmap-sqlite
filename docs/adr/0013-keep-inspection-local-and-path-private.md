# Keep inspection local and path-private

The inspector performs no telemetry or external network calls and does not expose absolute source paths in browser responses or URLs, using an opaque snapshot identity and separate display name instead. Explicitly selected typed application values may cross the operator's chosen web connection, but raw application payloads never do; this preserves remote-development usefulness while preventing filesystem details and broad record contents from leaking through routine navigation.
