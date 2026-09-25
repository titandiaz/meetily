import { useEffect, useState } from 'react';
import { toast } from 'sonner';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from './ui/select';
import { Label } from './ui/label';
import { useTranscriptionModels } from '@/hooks/useTranscriptionModels';
import { loadReprocessingModelKey, saveReprocessingModelKey } from '@/lib/reprocessingModel';

const SAME_AS_LIVE = 'same-as-live';

/** Picks the model used to re-transcribe meetings and import audio, independently of the live model. */
export function ReprocessingModelSetting() {
    const { availableModels, fetchModels, loadingModels } = useTranscriptionModels(undefined);
    const [selectedKey, setSelectedKey] = useState<string>(SAME_AS_LIVE);

    useEffect(() => {
        fetchModels();
        loadReprocessingModelKey().then(key => setSelectedKey(key ?? SAME_AS_LIVE));
    }, [fetchModels]);

    const handleChange = async (value: string) => {
        setSelectedKey(value);
        try {
            await saveReprocessingModelKey(value === SAME_AS_LIVE ? null : value);
            toast.success('Reprocessing model saved');
        } catch (error) {
            console.error('Failed to save reprocessing model:', error);
            toast.error('Failed to save reprocessing model');
        }
    };

    // Keep a saved model visible even if it was deleted since
    const savedIsMissing = selectedKey !== SAME_AS_LIVE
        && !loadingModels
        && !availableModels.some(m => `${m.provider}:${m.name}` === selectedKey);

    return (
        <div>
            <Label className="block text-sm font-medium text-gray-700 mb-1">Reprocessing model</Label>
            <Select
                value={selectedKey}
                onValueChange={handleChange}
                onOpenChange={open => { if (open) fetchModels(); }}
            >
                <SelectTrigger className='focus:ring-1 focus:ring-blue-500 focus:border-blue-500'>
                    <SelectValue placeholder="Same as live transcription" />
                </SelectTrigger>
                <SelectContent>
                    <SelectItem value={SAME_AS_LIVE}>Same as live transcription</SelectItem>
                    {availableModels.map(model => (
                        <SelectItem key={`${model.provider}:${model.name}`} value={`${model.provider}:${model.name}`}>
                            {model.displayName} ({Math.round(model.size_mb)} MB)
                        </SelectItem>
                    ))}
                    {savedIsMissing && (
                        <SelectItem value={selectedKey} disabled>{selectedKey} (not downloaded)</SelectItem>
                    )}
                </SelectContent>
            </Select>
            <p className="text-xs text-gray-500 mt-1">
                Default for re-transcribing meetings and importing audio. Choosing a model here never changes live transcription.
            </p>
        </div>
    );
}
