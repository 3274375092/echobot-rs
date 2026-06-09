export function normalizeSpeechProviders(items, fallbackKind) {
    return Array.isArray(items)
        ? items.map((item) => ({
            kind: String((item && item.kind) || fallbackKind || ""),
            name: String((item && item.name) || ""),
            label: String((item && item.label) || ""),
            selected: Boolean(item && item.selected),
            available: Boolean(item && item.available),
            state: String((item && item.state) || "missing"),
            detail: String((item && item.detail) || ""),
            resource_directory: String((item && item.resource_directory) || ""),
        }))
        : [];
}

export function normalizeAsrConfig(asrConfig) {
    return {
        available: Boolean(asrConfig && asrConfig.available),
        state: String((asrConfig && asrConfig.state) || "missing"),
        detail: String((asrConfig && asrConfig.detail) || ""),
        sample_rate: Number((asrConfig && asrConfig.sample_rate) || 16000),
        selected_asr_provider: String((asrConfig && asrConfig.selected_asr_provider) || ""),
        selected_vad_provider: String((asrConfig && asrConfig.selected_vad_provider) || ""),
        always_listen_supported: Boolean(
            asrConfig && Object.prototype.hasOwnProperty.call(asrConfig, "always_listen_supported")
                ? asrConfig.always_listen_supported
                : false,
        ),
        asr_providers: normalizeSpeechProviders(asrConfig && asrConfig.asr_providers, "asr"),
        vad_providers: normalizeSpeechProviders(asrConfig && asrConfig.vad_providers, "vad"),
    };
}

export function findAsrProviderStatus(asrConfig, providerName) {
    const providers = Array.isArray(asrConfig && asrConfig.asr_providers)
        ? asrConfig.asr_providers
        : [];
    return providers.find((item) => item.name === providerName) || null;
}
