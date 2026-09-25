import { useCallback, RefObject } from 'react';
import { MeetingSummary, Transcript } from '@/types';
import { BlockNoteSummaryViewRef } from '@/components/AISummary/BlockNoteSummaryView';
import { toast } from 'sonner';
import Analytics from '@/lib/analytics';
import { invoke as invokeTauri } from '@tauri-apps/api/core';
import { hasVisibleSummaryContent } from '@/lib/summary-content';
import {
  buildObsidianNote,
  chooseObsidianVault,
  formatTranscriptTime,
  loadObsidianSettings,
  openInObsidian,
} from '@/lib/obsidian';

interface UseCopyOperationsProps {
  meeting: any;
  transcripts: Transcript[];
  meetingTitle: string;
  aiSummary: MeetingSummary | null;
  blockNoteSummaryRef: RefObject<BlockNoteSummaryViewRef>;
}

export function useCopyOperations({
  meeting,
  transcripts,
  meetingTitle,
  aiSummary,
  blockNoteSummaryRef,
}: UseCopyOperationsProps) {

  // Helper function to fetch ALL transcripts for copying (not just paginated data)
  const fetchAllTranscripts = useCallback(async (meetingId: string): Promise<Transcript[]> => {
    try {
      console.log('📊 Fetching all transcripts for copying:', meetingId);

      // First, get total count by fetching first page
      const firstPage = await invokeTauri('api_get_meeting_transcripts', {
        meetingId,
        limit: 1,
        offset: 0,
      }) as { transcripts: Transcript[]; total_count: number; has_more: boolean };

      const totalCount = firstPage.total_count;
      console.log(`📊 Total transcripts in database: ${totalCount}`);

      if (totalCount === 0) {
        return [];
      }

      // Fetch all transcripts in one call
      const allData = await invokeTauri('api_get_meeting_transcripts', {
        meetingId,
        limit: totalCount,
        offset: 0,
      }) as { transcripts: Transcript[]; total_count: number; has_more: boolean };

      console.log(`✅ Fetched ${allData.transcripts.length} transcripts from database for copying`);
      return allData.transcripts;
    } catch (error) {
      console.error('❌ Error fetching all transcripts:', error);
      toast.error('Failed to fetch transcripts for copying');
      return [];
    }
  }, []);

  // Copy transcript to clipboard
  const handleCopyTranscript = useCallback(async () => {
    // CHANGE: Fetch ALL transcripts from database, not from pagination state
    console.log('📊 Fetching all transcripts for copying...');
    const allTranscripts = await fetchAllTranscripts(meeting.id);

    if (!allTranscripts.length) {
      const error_msg = 'No transcripts available to copy';
      console.log(error_msg);
      toast.error(error_msg);
      return;
    }

    console.log(`✅ Copying ${allTranscripts.length} transcripts to clipboard`);

    const header = `# Transcript of the Meeting: ${meeting.id} - ${meetingTitle ?? meeting.title}\n\n`;
    const date = `## Date: ${new Date(meeting.created_at).toLocaleDateString()}\n\n`;
    const fullTranscript = allTranscripts
      .map(t => `${formatTranscriptTime(t.audio_start_time, t.timestamp)} ${t.text}  `)
      .join('\n');

    await navigator.clipboard.writeText(header + date + fullTranscript);
    toast.success("Transcript copied to clipboard");

    // Track copy analytics
    const wordCount = allTranscripts
      .map(t => t.text.split(/\s+/).length)
      .reduce((a, b) => a + b, 0);

    await Analytics.trackCopy('transcript', {
      meeting_id: meeting.id,
      transcript_length: allTranscripts.length.toString(),
      word_count: wordCount.toString()
    });
  }, [meeting, meetingTitle, fetchAllTranscripts]);

  // Summary as markdown: BlockNote editor first, then stored markdown, then legacy sections
  const resolveSummaryMarkdown = useCallback(async (): Promise<string> => {
    let summaryMarkdown = '';

    console.log('🔍 Resolving summary markdown...');

    // Try to get markdown from BlockNote editor first
    if (blockNoteSummaryRef.current?.getMarkdown) {
      console.log('📝 Trying to get markdown from ref...');
      summaryMarkdown = await blockNoteSummaryRef.current.getMarkdown();
      console.log('📝 Got markdown from ref, length:', summaryMarkdown.length);
    }

    // Fallback: Check if aiSummary has markdown property
    if (!summaryMarkdown && aiSummary && typeof aiSummary.markdown === 'string') {
      console.log('📝 Using markdown from aiSummary');
      summaryMarkdown = aiSummary.markdown;
      console.log('📝 Markdown from aiSummary, length:', summaryMarkdown.length);
    }

    // Fallback: Check for legacy format
    if (!summaryMarkdown && aiSummary) {
      console.log('📝 Converting legacy format to markdown');
      const sections = Object.entries(aiSummary)
        .filter(([key]) => {
          // Skip non-section keys
          return key !== 'markdown' && key !== 'summary_json' && key !== '_section_order' && key !== 'MeetingName';
        })
        .map(([, section]) => {
          if (section && typeof section === 'object' && 'title' in section && 'blocks' in section) {
            const sectionTitle = `## ${section.title}\n\n`;
            const sectionContent = section.blocks
              .map((block: any) => `- ${block.content}`)
              .join('\n');
            return sectionTitle + sectionContent;
          }
          return '';
        })
        .filter(s => s.trim())
        .join('\n\n');
      summaryMarkdown = sections;
      console.log('📝 Converted legacy format, length:', summaryMarkdown.length);
    }

    return summaryMarkdown;
  }, [aiSummary, blockNoteSummaryRef]);

  // Copy summary to clipboard
  const handleCopySummary = useCallback(async () => {
    if (!hasVisibleSummaryContent(aiSummary)) {
      toast.error('No summary content available to copy');
      return;
    }
    try {
      const summaryMarkdown = await resolveSummaryMarkdown();

      // If still no summary content, show message
      if (!summaryMarkdown.trim()) {
        console.error('❌ No summary content available to copy');
        toast.error('No summary content available to copy');
        return;
      }

      // Build metadata header
      const header = `# Meeting Summary: ${meetingTitle}\n\n`;
      const metadata = `**Meeting ID:** ${meeting.id}\n**Date:** ${new Date(meeting.created_at).toLocaleDateString('en-US', {
        year: 'numeric',
        month: 'long',
        day: 'numeric',
        hour: '2-digit',
        minute: '2-digit'
      })}\n**Copied on:** ${new Date().toLocaleDateString('en-US', {
        year: 'numeric',
        month: 'long',
        day: 'numeric',
        hour: '2-digit',
        minute: '2-digit'
      })}\n\n---\n\n`;

      const fullMarkdown = header + metadata + summaryMarkdown;
      await navigator.clipboard.writeText(fullMarkdown);

      console.log('✅ Successfully copied to clipboard!');
      toast.success("Summary copied to clipboard");

      // Track copy analytics
      await Analytics.trackCopy('summary', {
        meeting_id: meeting.id,
        has_markdown: (!!aiSummary && 'markdown' in aiSummary).toString()
      });
    } catch (error) {
      console.error('❌ Failed to copy summary:', error);
      toast.error("Failed to copy summary");
    }
  }, [aiSummary, meetingTitle, meeting, resolveSummaryMarkdown]);

  // Export summary + transcript as a single note into the Obsidian vault
  const handleExportToObsidian = useCallback(async () => {
    try {
      let { vaultPath, folder } = await loadObsidianSettings();
      if (!vaultPath) {
        vaultPath = await chooseObsidianVault();
        if (!vaultPath) return;
      }

      const allTranscripts = await fetchAllTranscripts(meeting.id);
      const summaryMarkdown = hasVisibleSummaryContent(aiSummary) ? await resolveSummaryMarkdown() : '';
      if (!allTranscripts.length && !summaryMarkdown.trim()) {
        toast.error('Nothing to export yet — no summary or transcript');
        return;
      }

      const { fileName, content } = buildObsidianNote({
        meetingId: meeting.id,
        title: meetingTitle || meeting.title || 'Meeting',
        createdAt: new Date(meeting.created_at),
        summaryMarkdown,
        transcriptLines: allTranscripts.map(t => `${formatTranscriptTime(t.audio_start_time, t.timestamp)} ${t.text}`),
      });

      const notePath = await invokeTauri<string>('obsidian_export_note', {
        vaultPath,
        folder,
        fileName,
        meetingId: meeting.id,
        content,
      });

      toast.success('Exported to Obsidian', {
        description: `${folder}/${notePath.split(/[\\/]/).pop()}`,
        action: { label: 'Open', onClick: () => void openInObsidian(notePath) },
      });
      await Analytics.track('export_obsidian', {
        meeting_id: meeting.id,
        has_summary: (!!summaryMarkdown.trim()).toString(),
        transcript_length: allTranscripts.length.toString(),
      });
    } catch (error) {
      console.error('❌ Failed to export to Obsidian:', error);
      toast.error('Failed to export to Obsidian', {
        description: error instanceof Error ? error.message : String(error),
      });
    }
  }, [meeting, meetingTitle, aiSummary, fetchAllTranscripts, resolveSummaryMarkdown]);

  return {
    handleCopyTranscript,
    handleCopySummary,
    handleExportToObsidian,
  };
}
