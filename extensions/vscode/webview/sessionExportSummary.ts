import type { CommandResultView, ExportedArtifactView } from '../src/types';

export interface SessionExportSummary {
  generatedArtifacts: ExportedArtifactView[];
  fullCopyFiles: string[];
  chips: string[];
}

export function sessionExportSummary(result: CommandResultView | undefined): SessionExportSummary {
  const generatedArtifacts = Array.isArray(result?.artifacts) ? result.artifacts : [];
  const fullCopyFiles = Array.isArray(result?.files)
    ? result.files.filter((file): file is string => typeof file === 'string' && file.trim().length > 0)
    : [];
  const chips: string[] = [];
  if (generatedArtifacts.length > 0) {
    chips.push(`${generatedArtifacts.length} generated ${generatedArtifacts.length === 1 ? 'file' : 'files'}`);
  }
  if (fullCopyFiles.length > 0) {
    chips.push(`${fullCopyFiles.length} full-copy ${fullCopyFiles.length === 1 ? 'entry' : 'entries'}`);
  }
  if (chips.length === 0) chips.push('0 files');
  return { generatedArtifacts, fullCopyFiles, chips };
}
