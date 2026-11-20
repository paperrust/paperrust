// GENERATED from frontend/.build/src-ui/src/app/data/paperless-config.ts (options of the application configuration);
// $localize → msg() (displayed via t()). Regenerate instead of editing when upstream changes.
// Afterwards switch config_key to PAPERRUST_* (displayed name; PAPERLESS_* remains valid as an alias).
import { msg } from '../../../lib/i18n'

// see /src/paperless/models.py

export enum OutputTypeConfig {
  PDF = 'pdf',
  PDF_A = 'pdfa',
  PDF_A1 = 'pdfa-1',
  PDF_A2 = 'pdfa-2',
  PDF_A3 = 'pdfa-3',
}

export enum ModeConfig {
  AUTO = 'auto',
  FORCE = 'force',
  REDO = 'redo',
  OFF = 'off',
}

export enum ArchiveFileConfig {
  AUTO = 'auto',
  ALWAYS = 'always',
  NEVER = 'never',
}

export enum CleanConfig {
  CLEAN = 'clean',
  FINAL = 'clean-final',
  NONE = 'none',
}

export enum ColorConvertConfig {
  UNCHANGED = 'LeaveColorUnchanged',
  RGB = 'RGB',
  INDEPENDENT = 'UseDeviceIndependentColor',
  GRAY = 'Gray',
  CMYK = 'CMYK',
}

export enum ConfigOptionType {
  String = 'string',
  Number = 'number',
  Select = 'select',
  Boolean = 'boolean',
  JSON = 'json',
  File = 'file',
  Password = 'password',
}

export const ConfigCategory = {
  General: msg("admin.config.general_settings"),
  OCR: msg("admin.config.ocr_settings"),
  Barcode: msg("admin.config.barcode_settings"),
  AI: msg("admin.config.ai_settings"),
}

export const ConfigSection = {
  RemoteOCR: msg("common.remote_ocr"),
}

export const LLMEmbeddingBackendConfig = {
  OPENAI_LIKE: 'openai-like',
  HUGGINGFACE: 'huggingface',
  OLLAMA: 'ollama',
}

export const LLMBackendConfig = {
  OPENAI_LIKE: 'openai-like',
  OLLAMA: 'ollama',
}

export const RemoteOCREngineConfig = {
  AZURE_AI: 'azureai',
}

export const RemoteOCRModeConfig = {
  ALWAYS: 'always',
  WORKFLOW_ONLY: 'workflow_only',
}

export interface ConfigOption {
  key: string
  title: string
  type: ConfigOptionType
  choices?: Array<{ id: string; name: string }>
  config_key?: string
  category: string
  section?: string
  note?: string
}

function mapToItems(enumObj: any): Array<{ id: string; name: string }> {
  return Object.keys(enumObj).map((key) => {
    return {
      id: enumObj[key],
      name: enumObj[key],
    }
  })
}

/** Output language of the AI: selection instead of free text (feedback 2026-11-17); codes like the UI languages,
 *  pl-ai passes them on unchanged (get_llm_output_language). */
export const LLM_LANGUAGE_CHOICES: Array<{ id: string; name: string }> = [
  { id: 'de-de', name: 'Deutsch' }, { id: 'en-us', name: 'English' }, { id: 'fr-fr', name: 'Français' },
  { id: 'it-it', name: 'Italiano' }, { id: 'es-es', name: 'Español' }, { id: 'nl-nl', name: 'Nederlands' },
  { id: 'pl-pl', name: 'Polski' }, { id: 'cs-cz', name: 'Čeština' }, { id: 'pt-br', name: 'Português (Brasil)' },
  { id: 'sv-se', name: 'Svenska' },
]

export const PaperlessConfigOptions: ConfigOption[] = [
  {
    key: 'output_type',
    title: msg("admin.config.output_type"),
    type: ConfigOptionType.Select,
    choices: mapToItems(OutputTypeConfig),
    config_key: 'PAPERRUST_OCR_OUTPUT_TYPE',
    category: ConfigCategory.OCR,
  },
  {
    key: 'language',
    title: msg("common.language"),
    type: ConfigOptionType.String,
    config_key: 'PAPERRUST_OCR_LANGUAGE',
    category: ConfigCategory.OCR,
  },
  {
    key: 'pages',
    title: msg("common.pages"),
    type: ConfigOptionType.Number,
    config_key: 'PAPERRUST_OCR_PAGES',
    category: ConfigCategory.OCR,
  },
  {
    key: 'mode',
    title: msg("admin.config.mode"),
    type: ConfigOptionType.Select,
    choices: mapToItems(ModeConfig),
    config_key: 'PAPERRUST_OCR_MODE',
    category: ConfigCategory.OCR,
  },
  {
    key: 'archive_file_generation',
    title: msg("admin.config.archive_file_generation"),
    type: ConfigOptionType.Select,
    choices: mapToItems(ArchiveFileConfig),
    config_key: 'PAPERRUST_ARCHIVE_FILE_GENERATION',
    category: ConfigCategory.OCR,
  },
  {
    key: 'image_dpi',
    title: msg("admin.config.image_dpi"),
    type: ConfigOptionType.Number,
    config_key: 'PAPERRUST_OCR_IMAGE_DPI',
    category: ConfigCategory.OCR,
  },
  {
    key: 'unpaper_clean',
    title: msg("admin.config.clean"),
    type: ConfigOptionType.Select,
    choices: mapToItems(CleanConfig),
    config_key: 'PAPERRUST_OCR_CLEAN',
    category: ConfigCategory.OCR,
  },
  {
    key: 'deskew',
    title: msg("admin.config.deskew"),
    type: ConfigOptionType.Boolean,
    config_key: 'PAPERRUST_OCR_DESKEW',
    category: ConfigCategory.OCR,
  },
  {
    key: 'rotate_pages',
    title: msg("admin.config.rotate_pages"),
    type: ConfigOptionType.Boolean,
    config_key: 'PAPERRUST_OCR_ROTATE_PAGES',
    category: ConfigCategory.OCR,
  },
  {
    key: 'rotate_pages_threshold',
    title: msg("admin.config.rotate_pages_threshold"),
    type: ConfigOptionType.Number,
    config_key: 'PAPERRUST_OCR_ROTATE_PAGES_THRESHOLD',
    category: ConfigCategory.OCR,
  },
  {
    key: 'max_image_pixels',
    title: msg("admin.config.max_image_pixels"),
    type: ConfigOptionType.Number,
    config_key: 'PAPERRUST_OCR_MAX_IMAGE_PIXELS',
    category: ConfigCategory.OCR,
  },
  {
    key: 'color_conversion_strategy',
    title: msg("admin.config.color_conversion_strategy"),
    type: ConfigOptionType.Select,
    choices: mapToItems(ColorConvertConfig),
    config_key: 'PAPERRUST_OCR_COLOR_CONVERSION_STRATEGY',
    category: ConfigCategory.OCR,
  },
  {
    key: 'user_args',
    title: msg("admin.config.ocr_arguments"),
    type: ConfigOptionType.JSON,
    config_key: 'PAPERRUST_OCR_USER_ARGS',
    category: ConfigCategory.OCR,
  },
  {
    key: 'remote_ocr_engine',
    title: msg("admin.config.remote_ocr_engine"),
    type: ConfigOptionType.Select,
    choices: mapToItems(RemoteOCREngineConfig),
    config_key: 'PAPERRUST_REMOTE_OCR_ENGINE',
    category: ConfigCategory.OCR,
    section: ConfigSection.RemoteOCR,
    note: msg("admin.config.enabling_remote_ocr_sends"),
  },
  {
    key: 'remote_ocr_api_key',
    title: msg("admin.config.remote_ocr_api_key"),
    type: ConfigOptionType.Password,
    config_key: 'PAPERRUST_REMOTE_OCR_API_KEY',
    category: ConfigCategory.OCR,
    section: ConfigSection.RemoteOCR,
  },
  {
    key: 'remote_ocr_endpoint',
    title: msg("admin.config.remote_ocr_endpoint"),
    type: ConfigOptionType.String,
    config_key: 'PAPERRUST_REMOTE_OCR_ENDPOINT',
    category: ConfigCategory.OCR,
    section: ConfigSection.RemoteOCR,
    note: msg("admin.config.required_using_azure_ai"),
  },
  {
    key: 'remote_ocr_mode',
    title: msg("admin.config.remote_ocr_mode"),
    type: ConfigOptionType.Select,
    choices: mapToItems(RemoteOCRModeConfig),
    config_key: 'PAPERRUST_REMOTE_OCR_MODE',
    category: ConfigCategory.OCR,
    section: ConfigSection.RemoteOCR,
    note: msg("admin.config.documents_sent_remote_engine"),
  },
  {
    key: 'app_logo',
    title: msg("admin.config.application_logo"),
    type: ConfigOptionType.File,
    config_key: 'PAPERRUST_APP_LOGO',
    category: ConfigCategory.General,
  },
  {
    key: 'app_title',
    title: msg("admin.config.application_title"),
    type: ConfigOptionType.String,
    config_key: 'PAPERRUST_APP_TITLE',
    category: ConfigCategory.General,
  },
  {
    key: 'barcodes_enabled',
    title: msg("admin.config.enable_barcodes"),
    type: ConfigOptionType.Boolean,
    config_key: 'PAPERRUST_CONSUMER_ENABLE_BARCODES',
    category: ConfigCategory.Barcode,
  },
  {
    key: 'barcode_enable_tiff_support',
    title: msg("admin.config.enable_tiff_support"),
    type: ConfigOptionType.Boolean,
    config_key: 'PAPERRUST_CONSUMER_BARCODE_TIFF_SUPPORT',
    category: ConfigCategory.Barcode,
  },
  {
    key: 'barcode_string',
    title: msg("admin.config.barcode_string"),
    type: ConfigOptionType.String,
    config_key: 'PAPERRUST_CONSUMER_BARCODE_STRING',
    category: ConfigCategory.Barcode,
  },
  {
    key: 'barcode_retain_split_pages',
    title: msg("admin.config.retain_split_pages"),
    type: ConfigOptionType.Boolean,
    config_key: 'PAPERRUST_CONSUMER_BARCODE_RETAIN_SPLIT_PAGES',
    category: ConfigCategory.Barcode,
  },
  {
    key: 'barcode_enable_asn',
    title: msg("admin.config.enable_asn"),
    type: ConfigOptionType.Boolean,
    config_key: 'PAPERRUST_CONSUMER_ENABLE_ASN_BARCODE',
    category: ConfigCategory.Barcode,
  },
  {
    key: 'barcode_asn_prefix',
    title: msg("admin.config.asn_prefix"),
    type: ConfigOptionType.String,
    config_key: 'PAPERRUST_CONSUMER_ASN_BARCODE_PREFIX',
    category: ConfigCategory.Barcode,
  },
  {
    key: 'barcode_upscale',
    title: msg("admin.config.upscale"),
    type: ConfigOptionType.Number,
    config_key: 'PAPERRUST_CONSUMER_BARCODE_UPSCALE',
    category: ConfigCategory.Barcode,
  },
  {
    key: 'barcode_dpi',
    title: msg("admin.config.dpi"),
    type: ConfigOptionType.Number,
    config_key: 'PAPERRUST_CONSUMER_BARCODE_DPI',
    category: ConfigCategory.Barcode,
  },
  {
    key: 'barcode_max_pages',
    title: msg("admin.config.max_pages"),
    type: ConfigOptionType.Number,
    config_key: 'PAPERRUST_CONSUMER_BARCODE_MAX_PAGES',
    category: ConfigCategory.Barcode,
  },
  {
    key: 'barcode_enable_tag',
    title: msg("admin.config.enable_tag_detection"),
    type: ConfigOptionType.Boolean,
    config_key: 'PAPERRUST_CONSUMER_ENABLE_TAG_BARCODE',
    category: ConfigCategory.Barcode,
  },
  {
    key: 'barcode_tag_mapping',
    title: msg("admin.config.tag_mapping"),
    type: ConfigOptionType.JSON,
    config_key: 'PAPERRUST_CONSUMER_TAG_BARCODE_MAPPING',
    category: ConfigCategory.Barcode,
  },
  {
    key: 'barcode_tag_split',
    title: msg("admin.config.split_tag_barcodes"),
    type: ConfigOptionType.Boolean,
    config_key: 'PAPERRUST_CONSUMER_TAG_BARCODE_SPLIT',
    category: ConfigCategory.Barcode,
  },
  {
    key: 'barcode_store_values',
    title: msg("admin.config.store_barcode_contents"),
    type: ConfigOptionType.Boolean,
    config_key: 'PAPERRUST_CONSUMER_STORE_BARCODE_VALUES',
    category: ConfigCategory.Barcode,
  },
  {
    key: 'ai_enabled',
    title: msg("admin.config.ai_enabled"),
    type: ConfigOptionType.Boolean,
    config_key: 'PAPERRUST_AI_ENABLED',
    category: ConfigCategory.AI,
    note: msg("admin.config.consider_privacy_implications_enabling"),
  },
  {
    key: 'llm_embedding_backend',
    title: msg("admin.config.llm_embedding_backend"),
    type: ConfigOptionType.Select,
    choices: mapToItems(LLMEmbeddingBackendConfig),
    config_key: 'PAPERRUST_AI_LLM_EMBEDDING_BACKEND',
    category: ConfigCategory.AI,
  },
  {
    key: 'llm_embedding_model',
    title: msg("admin.config.llm_embedding_model"),
    type: ConfigOptionType.String,
    config_key: 'PAPERRUST_AI_LLM_EMBEDDING_MODEL',
    category: ConfigCategory.AI,
  },
  {
    key: 'llm_embedding_api_key',
    title: msg("admin.config.llm_embedding_api_key"),
    type: ConfigOptionType.Password,
    note: msg("admin.config.used_embeddings_set_otherwise"),
    config_key: 'PAPERRUST_AI_LLM_EMBEDDING_API_KEY',
    category: ConfigCategory.AI,
  },
  {
    key: 'llm_embedding_endpoint',
    title: msg("admin.config.llm_embedding_endpoint"),
    type: ConfigOptionType.String,
    config_key: 'PAPERRUST_AI_LLM_EMBEDDING_ENDPOINT',
    category: ConfigCategory.AI,
  },
  {
    key: 'llm_embedding_chunk_size',
    title: msg("admin.config.llm_embedding_chunk_size"),
    type: ConfigOptionType.Number,
    config_key: 'PAPERRUST_AI_LLM_EMBEDDING_CHUNK_SIZE',
    category: ConfigCategory.AI,
  },
  {
    key: 'llm_context_size',
    title: msg("admin.config.llm_context_size"),
    type: ConfigOptionType.Number,
    config_key: 'PAPERRUST_AI_LLM_CONTEXT_SIZE',
    category: ConfigCategory.AI,
  },
  {
    key: 'llm_backend',
    title: msg("admin.config.llm_backend"),
    type: ConfigOptionType.Select,
    choices: mapToItems(LLMBackendConfig),
    config_key: 'PAPERRUST_AI_LLM_BACKEND',
    category: ConfigCategory.AI,
  },
  {
    key: 'llm_model',
    title: msg("admin.config.llm_model"),
    type: ConfigOptionType.String,
    config_key: 'PAPERRUST_AI_LLM_MODEL',
    category: ConfigCategory.AI,
  },
  {
    key: 'llm_api_key',
    title: msg("admin.config.llm_api_key"),
    type: ConfigOptionType.Password,
    config_key: 'PAPERRUST_AI_LLM_API_KEY',
    category: ConfigCategory.AI,
  },
  {
    key: 'llm_endpoint',
    title: msg("admin.config.llm_endpoint"),
    type: ConfigOptionType.String,
    config_key: 'PAPERRUST_AI_LLM_ENDPOINT',
    category: ConfigCategory.AI,
  },
  {
    key: 'llm_output_language',
    title: msg("admin.config.llm_output_language"),
    type: ConfigOptionType.Select,
    choices: LLM_LANGUAGE_CHOICES,
    config_key: 'PAPERRUST_AI_LLM_OUTPUT_LANGUAGE',
    category: ConfigCategory.AI,
    note: msg("admin.config.language_use_generated_ai"),
  },
  {
    key: 'llm_request_timeout',
    title: msg("admin.config.llm_request_timeout"),
    type: ConfigOptionType.Number,
    config_key: 'PAPERRUST_AI_LLM_REQUEST_TIMEOUT',
    category: ConfigCategory.AI,
    note: msg("admin.config.timeout_in_seconds_llm"),
  },
]
