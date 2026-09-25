/**
 * Model used for batch work (re-transcribing a meeting, importing audio),
 * kept separate from the live-transcription model in transcript_settings so
 * that picking a slow, accurate model for reprocessing never changes what
 * runs in real time. Stored as the `provider:name` key used by
 * useTranscriptionModels; null means "same as live transcription".
 */
const REPROCESSING_MODEL_KEY = 'reprocessing_model_key';

async function loadStore() {
  const { Store } = await import('@tauri-apps/plugin-store');
  return Store.load('preferences.json');
}

export async function loadReprocessingModelKey(): Promise<string | null> {
  try {
    const store = await loadStore();
    return (await store.get<string>(REPROCESSING_MODEL_KEY)) || null;
  } catch (error) {
    console.error('Failed to load reprocessing model preference:', error);
    return null;
  }
}

export async function saveReprocessingModelKey(key: string | null): Promise<void> {
  const store = await loadStore();
  await store.set(REPROCESSING_MODEL_KEY, key);
  await store.save();
}
