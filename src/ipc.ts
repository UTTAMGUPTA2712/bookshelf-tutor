// Typed wrappers around the Rust commands (src-tauri/src/commands.rs).
import { Channel, invoke } from "@tauri-apps/api/core";

export interface AppError {
  kind: string;
  message: string;
}

export function errorMessage(e: unknown): string {
  if (e && typeof e === "object" && "message" in e) return String((e as AppError).message);
  return String(e);
}

export interface Track {
  id: string;
  label: string;
}

export interface AppInfo {
  deviceId: string;
  libraryRoot: string | null;
  dataDir: string;
  version: string;
  pdfiumAvailable: boolean;
  tracks: Track[];
}

export interface BookProgress {
  status: string;
  lastPage: number;
  chaptersDone: string[];
}

export interface Book {
  fingerprint: string;
  title: string;
  author: string | null;
  track: string;
  status: BookStatus;
  level: string | null;
  topics: string[];
  fileName: string;
  size: number;
  pageCount: number | null;
  path: string | null;
  progress: BookProgress | null;
}

export type BookStatus = "start-here" | "core" | "reference" | "later" | "outdated";

export const BOOK_STATUSES: { id: BookStatus; label: string }[] = [
  { id: "start-here", label: "Start here" },
  { id: "core", label: "Core" },
  { id: "reference", label: "Reference" },
  { id: "later", label: "Later" },
  { id: "outdated", label: "Outdated" },
];

export interface AddResult {
  added: string[];
  duplicates: { file: string; existingTitle: string }[];
  errors: string[];
}

export interface IndexStatus {
  indexed: boolean;
  pageCount: number;
  textPages: number;
  chapters: number;
}

export interface Chapter {
  idx: number;
  id: string;
  title: string;
  depth: number;
  startPage: number;
  endPage: number;
}

export interface SearchHit {
  chunkId: number;
  chapterIdx: number;
  pageStart: number;
  pageEnd: number;
  snippet: string;
}

export interface Preset {
  type: string;
  label: string;
  adapter: "ollama" | "openai";
  baseUrl: string;
  needsKey: boolean;
  local: boolean;
  docsUrl: string;
  note: string;
}

export interface ProviderOptions {
  numCtx?: number | null;
  numThread?: number | null;
  temperature?: number | null;
}

export interface Provider {
  id: string;
  providerType: string;
  label: string;
  baseUrl: string;
  enabledModels: string[];
  options: ProviderOptions;
  adapter: string;
  needsKey: boolean;
  hasKey: boolean;
}

export interface ProviderInput {
  id?: string;
  providerType: string;
  label: string;
  baseUrl: string;
  enabledModels: string[];
  options: ProviderOptions;
  apiKey?: string;
}

export interface ModelInfo {
  id: string;
  name: string | null;
  contextLength: number | null;
  promptPrice: number | null;
  completionPrice: number | null;
  sizeBytes: number | null;
  parameterSize: string | null;
  quantization: string | null;
}

export interface RoleLink {
  providerId: string;
  model: string;
}

export interface Role {
  role: string;
  label: string;
  chain: RoleLink[];
}

export interface TestResult {
  latencyMs: number;
  jsonOk: boolean;
  reply: string;
  usage: { promptTokens: number | null; completionTokens: number | null };
}

export interface Source {
  label: string;
  pageStart: number;
  pageEnd: number;
  excerpt: string;
}

export interface Grade {
  score: number;
  verdict: "correct" | "partial" | "incorrect";
  correctPoints: string[];
  missingPoints: string[];
  misconceptions: string[];
  feedback: string;
  rereadPages: number[];
  gradedBy: unknown;
  gradedAt: string;
}

export interface Question {
  id: string;
  type: "mcq" | "short" | "explain";
  prompt: string;
  choices: string[] | null;
  correctChoice: number | null;
  referenceAnswer: string;
  keyPoints: string[];
  sources: Source[];
  answer: string | null;
  grade: Grade | null;
}

export interface Overall {
  score: number;
  summary: string;
  strengths: string[];
  focusNext: string[];
  nextStep: "move_on" | "review_weak_spots" | "reread_chapter";
  rereadPages: number[];
}

export interface Attempt {
  id: string;
  kind: string;
  bookFingerprint: string | null;
  chapterId: string | null;
  chapterTitle: string | null;
  status: "in_progress" | "completed";
  model: { question?: { provider: string; model: string } };
  questions: Question[];
  overall: Overall | null;
  createdAt: string;
  updatedAt: string;
}

export interface AttemptSummary {
  id: string;
  bookFingerprint: string | null;
  bookTitle: string | null;
  chapterId: string | null;
  chapterTitle: string | null;
  status: string;
  score: number | null;
  questionCount: number;
  answered: number;
  createdAt: string;
}

export interface QuizOptions {
  count: number;
  difficulty: "easy" | "medium" | "hard";
  types: ("mcq" | "short" | "explain")[];
}

export type StreamEvent = { type: "token"; text: string } | { type: "status"; message: string };

export interface TableChanges {
  table: string;
  added: number;
  updated: number;
  unchanged: number;
}

export interface ImportPreview {
  fromDevice: string;
  sameDevice: boolean;
  exportedAt: string;
  appVersion: string;
  changes: TableChanges[];
}

function channel<T>(onMessage?: (m: T) => void): Channel<T> {
  const ch = new Channel<T>();
  if (onMessage) ch.onmessage = onMessage;
  return ch;
}

export const newRequestId = () => crypto.randomUUID();

export const api = {
  appInfo: () => invoke<AppInfo>("app_info"),
  setLibraryRoot: (path: string) => invoke<Book[]>("set_library_root", { path }),
  scanLibrary: () => invoke<Book[]>("scan_library"),
  listBooks: () => invoke<Book[]>("list_books"),
  addBooks: (paths: string[], mode: "copy" | "link") => invoke<AddResult>("add_books", { paths, mode }),
  updateBook: (fingerprint: string, patch: Partial<Pick<Book, "title" | "author" | "track" | "status" | "level">>) =>
    invoke<Book>("update_book", { fingerprint, patch }),
  removeBook: (fingerprint: string, deleteFile: boolean, deleteProgress: boolean) =>
    invoke<void>("remove_book", { fingerprint, deleteFile, deleteProgress }),
  readBookFile: (fingerprint: string) => invoke<ArrayBuffer>("read_book_file", { fingerprint }),
  saveProgress: (
    fingerprint: string,
    patch: { lastPage?: number; status?: string; chapterDone?: string; chapterUndone?: string },
  ) => invoke<BookProgress>("save_progress", { fingerprint, patch }),

  indexStatus: (fingerprint: string) => invoke<IndexStatus>("index_status", { fingerprint }),
  indexBook: (fingerprint: string, force: boolean, onProgress: (p: { page: number; total: number }) => void) =>
    invoke<IndexStatus>("index_book", { fingerprint, force, onProgress: channel(onProgress) }),
  bookChapters: (fingerprint: string) => invoke<Chapter[]>("book_chapters", { fingerprint }),
  searchBook: (fingerprint: string, query: string, chapterIdx?: number) =>
    invoke<SearchHit[]>("search_book", { fingerprint, query, chapterIdx: chapterIdx ?? null }),

  providerPresets: () => invoke<Preset[]>("provider_presets"),
  providers: () => invoke<Provider[]>("providers_list"),
  saveProvider: (input: ProviderInput) => invoke<Provider>("provider_save", { input }),
  deleteProvider: (id: string) => invoke<void>("provider_delete", { id }),
  providerModels: (id: string) => invoke<ModelInfo[]>("provider_models", { id }),
  testModel: (providerId: string, model: string) => invoke<TestResult>("provider_test", { providerId, model }),
  roles: () => invoke<Role[]>("roles_get"),
  setRole: (role: string, chain: RoleLink[]) => invoke<void>("role_set", { role, chain }),

  cancel: (requestId: string) => invoke<void>("ai_cancel", { requestId }),
  generateQuiz: (
    requestId: string,
    fingerprint: string,
    chapterIdx: number,
    options: QuizOptions,
    onEvent: (e: StreamEvent) => void,
  ) => invoke<Attempt>("quiz_generate", { requestId, fingerprint, chapterIdx, options, onEvent: channel(onEvent) }),
  gradeAnswer: (
    requestId: string,
    attemptId: string,
    questionId: string,
    answer: string,
    onEvent: (e: StreamEvent) => void,
  ) => invoke<Attempt>("quiz_grade", { requestId, attemptId, questionId, answer, onEvent: channel(onEvent) }),
  finishQuiz: (requestId: string, attemptId: string, onEvent: (e: StreamEvent) => void) =>
    invoke<Attempt>("quiz_finish", { requestId, attemptId, onEvent: channel(onEvent) }),
  attempts: (fingerprint?: string) => invoke<AttemptSummary[]>("attempts_list", { fingerprint: fingerprint ?? null }),
  attempt: (id: string) => invoke<Attempt>("attempt_get", { id }),
  deleteAttempt: (id: string) => invoke<void>("attempt_delete", { id }),

  exportProgress: (path: string) => invoke<void>("export_progress", { path }),
  importPreview: (path: string) => invoke<ImportPreview>("import_preview", { path }),
  importApply: (path: string, mode: "merge" | "replace") => invoke<TableChanges[]>("import_apply", { path, mode }),
};
