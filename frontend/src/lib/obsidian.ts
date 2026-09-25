import { invoke } from '@tauri-apps/api/core';

export const DEFAULT_OBSIDIAN_FOLDER = 'meeting-notes';

const VAULT_KEY = 'obsidian_vault_path';
const FOLDER_KEY = 'obsidian_folder';

export interface ObsidianSettings {
  vaultPath: string | null;
  folder: string;
}

export interface ObsidianVault {
  path: string;
  name: string;
  open: boolean;
}

async function loadStore() {
  const { Store } = await import('@tauri-apps/plugin-store');
  return Store.load('preferences.json');
}

export async function loadObsidianSettings(): Promise<ObsidianSettings> {
  const store = await loadStore();
  const vaultPath = (await store.get<string>(VAULT_KEY)) ?? null;
  const folder = (await store.get<string>(FOLDER_KEY))?.trim() || DEFAULT_OBSIDIAN_FOLDER;
  return { vaultPath, folder };
}

export async function saveObsidianSettings(settings: Partial<ObsidianSettings>): Promise<void> {
  const store = await loadStore();
  if (settings.vaultPath !== undefined) {
    await store.set(VAULT_KEY, settings.vaultPath);
  }
  if (settings.folder !== undefined) {
    await store.set(FOLDER_KEY, settings.folder.trim() || DEFAULT_OBSIDIAN_FOLDER);
  }
  await store.save();
}

export function detectObsidianVaults(): Promise<ObsidianVault[]> {
  return invoke<ObsidianVault[]>('obsidian_detect_vaults');
}

/** Opens the native folder picker; persists and returns the chosen vault (null if cancelled). */
export async function chooseObsidianVault(): Promise<string | null> {
  const path = await invoke<string | null>('obsidian_select_vault');
  if (path) {
    await saveObsidianSettings({ vaultPath: path });
  }
  return path;
}

export async function openInObsidian(notePath: string): Promise<void> {
  await invoke('open_external_url', { url: `obsidian://open?path=${encodeURIComponent(notePath)}` });
}

function pad(n: number): string {
  return n.toString().padStart(2, '0');
}

export function formatTranscriptTime(seconds: number | undefined, fallbackTimestamp: string): string {
  if (seconds === undefined) {
    // Old transcripts without audio_start_time only have wall-clock time
    return fallbackTimestamp;
  }
  const totalSecs = Math.floor(seconds);
  return `[${pad(Math.floor(totalSecs / 60))}:${pad(totalSecs % 60)}]`;
}

/** Nest the summary's headings under the note's "## Summary" section. */
function demoteHeadings(markdown: string, levels: number): string {
  let inFence = false;
  return markdown
    .split('\n')
    .map(line => {
      if (/^\s*(```|~~~)/.test(line)) inFence = !inFence;
      if (inFence) return line;
      return line.replace(/^(#{1,6})(\s)/, (_, hashes: string, space: string) =>
        '#'.repeat(Math.min(hashes.length + levels, 6)) + space);
    })
    .join('\n');
}

interface ObsidianNoteInput {
  meetingId: string;
  title: string;
  createdAt: Date;
  summaryMarkdown: string | null;
  transcriptLines: string[];
}

export function buildObsidianNote({ meetingId, title, createdAt, summaryMarkdown, transcriptLines }: ObsidianNoteInput): {
  fileName: string;
  content: string;
} {
  const date = `${createdAt.getFullYear()}-${pad(createdAt.getMonth() + 1)}-${pad(createdAt.getDate())}`;
  const time = `${pad(createdAt.getHours())}:${pad(createdAt.getMinutes())}`;

  // JSON strings are valid YAML scalars, so this escapes quotes/colons safely
  const frontmatter = [
    '---',
    `title: ${JSON.stringify(title)}`,
    `date: ${date}`,
    `time: "${time}"`,
    `meetily_id: ${JSON.stringify(meetingId)}`,
    'tags:',
    '  - meeting',
    '---',
  ].join('\n');

  const sections = [`# ${title}`];
  if (summaryMarkdown?.trim()) {
    sections.push(`## Summary\n\n${demoteHeadings(summaryMarkdown.trim(), 2)}`);
  }
  if (transcriptLines.length > 0) {
    sections.push(`## Transcript\n\n${transcriptLines.join('\n')}`);
  }

  return {
    fileName: `${date} ${title}`,
    content: `${frontmatter}\n\n${sections.join('\n\n')}\n`,
  };
}
