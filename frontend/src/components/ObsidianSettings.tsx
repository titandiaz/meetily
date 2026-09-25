"use client"

import { useEffect, useState } from "react"
import { FolderOpen } from "lucide-react"
import { toast } from "sonner"
import { Input } from "./ui/input"
import {
  DEFAULT_OBSIDIAN_FOLDER,
  ObsidianVault,
  chooseObsidianVault,
  detectObsidianVaults,
  loadObsidianSettings,
  saveObsidianSettings,
} from "@/lib/obsidian"

export function ObsidianSettings() {
  const [vaultPath, setVaultPath] = useState<string | null>(null);
  const [folder, setFolder] = useState(DEFAULT_OBSIDIAN_FOLDER);
  const [detectedVaults, setDetectedVaults] = useState<ObsidianVault[]>([]);

  useEffect(() => {
    loadObsidianSettings()
      .then(settings => {
        setVaultPath(settings.vaultPath);
        setFolder(settings.folder);
      })
      .catch(error => console.error('Failed to load Obsidian settings:', error));
    detectObsidianVaults()
      .then(setDetectedVaults)
      .catch(error => console.error('Failed to detect Obsidian vaults:', error));
  }, []);

  const selectVault = async (path: string) => {
    try {
      await saveObsidianSettings({ vaultPath: path });
      setVaultPath(path);
      toast.success('Obsidian vault saved');
    } catch (error) {
      console.error('Failed to save Obsidian vault:', error);
      toast.error('Failed to save Obsidian vault');
    }
  };

  const handleBrowse = async () => {
    try {
      const path = await chooseObsidianVault();
      if (path) {
        setVaultPath(path);
        toast.success('Obsidian vault saved');
      }
    } catch (error) {
      console.error('Failed to choose Obsidian vault:', error);
      toast.error('Failed to choose Obsidian vault');
    }
  };

  const handleFolderBlur = async () => {
    const normalized = folder.trim() || DEFAULT_OBSIDIAN_FOLDER;
    setFolder(normalized);
    try {
      await saveObsidianSettings({ folder: normalized });
    } catch (error) {
      console.error('Failed to save Obsidian folder:', error);
      toast.error('Failed to save Obsidian folder');
    }
  };

  const otherVaults = detectedVaults.filter(v => v.path !== vaultPath);

  return (
    <div className="bg-white rounded-lg border border-gray-200 p-6 shadow-sm">
      <h3 className="text-lg font-semibold text-gray-900 mb-2">Obsidian Export</h3>
      <p className="text-sm text-gray-600 mb-6">
        The Obsidian button on a meeting saves its summary and transcript as a note in your vault
      </p>

      <div className="space-y-4">
        <div className="p-4 border rounded-lg bg-gray-50">
          <div className="font-medium mb-2">Vault</div>
          <div className="text-sm text-gray-600 mb-3 break-all font-mono text-xs">
            {vaultPath || 'Not set — you will be asked on the first export'}
          </div>
          <div className="flex flex-wrap gap-2">
            <button
              onClick={handleBrowse}
              className="flex items-center gap-2 px-3 py-2 text-sm border border-gray-300 rounded-md hover:bg-gray-100 transition-colors"
            >
              <FolderOpen className="w-4 h-4" />
              Choose Folder
            </button>
            {otherVaults.map(vault => (
              <button
                key={vault.path}
                onClick={() => selectVault(vault.path)}
                title={vault.path}
                className="px-3 py-2 text-sm border border-gray-300 rounded-md hover:bg-gray-100 transition-colors"
              >
                Use “{vault.name}”
              </button>
            ))}
          </div>
        </div>

        <div className="p-4 border rounded-lg bg-gray-50">
          <label htmlFor="obsidian-folder" className="font-medium mb-2 block">Folder inside the vault</label>
          <Input
            id="obsidian-folder"
            value={folder}
            onChange={e => setFolder(e.target.value)}
            onBlur={handleFolderBlur}
            placeholder={DEFAULT_OBSIDIAN_FOLDER}
            className="bg-white font-mono text-sm"
          />
        </div>
      </div>
    </div>
  );
}
