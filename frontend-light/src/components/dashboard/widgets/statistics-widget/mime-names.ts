// Excerpt from the package mime-names (Angular: getFileTypeName/getFileTypeExtension) for the file types that
// PaperRust accepts. Unknown types show the MIME type itself, like Angular.
export const MIME_NAMES: Record<string, [name: string, ext: string]> = {
  'application/pdf': ['Adobe Portable Document Format', 'pdf'],
  'image/png': ['Portable Network Graphics (PNG)', 'png'],
  'image/jpeg': ['JPEG Image', 'jpeg'],
  'image/tiff': ['Tagged Image File Format', 'tiff'],
  'image/gif': ['Graphics Interchange Format', 'gif'],
  'image/webp': ['WebP Image', 'webp'],
  'image/bmp': ['Bitmap Image File', 'bmp'],
  'text/plain': ['Text File', 'txt'],
  'text/csv': ['Comma-Separated Values', 'csv'],
  'text/html': ['HyperText Markup Language (HTML)', 'html'],
  'application/rtf': ['Rich Text Format', 'rtf'],
  'message/rfc822': ['Email Message', 'eml'],
  'application/msword': ['Microsoft Word', 'doc'],
  'application/vnd.openxmlformats-officedocument.wordprocessingml.document': ['Microsoft Office - OOXML - Word Document', 'docx'],
  'application/vnd.oasis.opendocument.text': ['OpenDocument Text', 'odt'],
  'application/vnd.ms-excel': ['Microsoft Excel', 'xls'],
  'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet': ['Microsoft Office - OOXML - Spreadsheet', 'xlsx'],
  'application/vnd.oasis.opendocument.spreadsheet': ['OpenDocument Spreadsheet', 'ods'],
  'application/vnd.ms-powerpoint': ['Microsoft PowerPoint', 'ppt'],
  'application/vnd.openxmlformats-officedocument.presentationml.presentation': ['Microsoft Office - OOXML - Presentation', 'pptx'],
  'application/vnd.oasis.opendocument.presentation': ['OpenDocument Presentation', 'odp'],
}

export const fileTypeName = (mime: string) => MIME_NAMES[mime]?.[0] ?? mime
export const fileTypeExtension = (mime: string) => MIME_NAMES[mime]?.[1]?.toUpperCase() ?? mime

export interface DocumentFileType {
  mime_type: string
  mime_type_count: number
  is_other?: boolean
}

export interface Statistics {
  documents_total?: number
  documents_inbox?: number | null
  inbox_tags?: number[]
  document_file_type_counts?: DocumentFileType[]
  character_count?: number
  tag_count?: number
  correspondent_count?: number
  document_type_count?: number
  storage_path_count?: number
  current_asn?: number
}

/** StatisticsWidgetComponent.reload: more than 5 file types → summarize the rest as "Other" */
export function groupFileTypes(statistics: Statistics, otherLabel: string, fileTypeMax = 5): Statistics {
  const counts = statistics.document_file_type_counts
  if (counts && counts.length > fileTypeMax) {
    const others = counts.slice(fileTypeMax)
    statistics.document_file_type_counts = [
      ...counts.slice(0, fileTypeMax),
      { mime_type: otherLabel, is_other: true, mime_type_count: others.reduce((n, f) => f.mime_type_count + n, 0) },
    ]
  }
  return statistics
}
