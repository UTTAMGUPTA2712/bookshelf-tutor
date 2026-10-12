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
  repeatPenalty?: number | null;
  repeatLastN?: number | null;
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

export interface SystemInfo {
  os: string;
  arch: string;
  cpuCores: number;
  totalRamGb: number;
  availableRamGb: number;
  accelerator: "apple-silicon" | "nvidia" | "cpu";
  usableForModelsGb: number;
}

export type Fit = "fits" | "tight" | "too-big";

export interface CatalogModel {
  name: string;
  label: string;
  sizeGb: number;
  memoryNeededGb: number;
  quality: number;
  goodFor: string[];
  note: string;
  fit: Fit;
  estTokensPerSec: number;
  estGradeSeconds: number;
  installed: boolean;
  recommended: boolean;
}

export interface CatalogView {
  system: SystemInfo;
  verifiedAt: string;
  models: CatalogModel[];
}

export interface LookupResult {
  name: string;
  sizeGb: number;
  memoryNeededGb: number;
  fit: Fit;
}

export interface PullProgress {
  status: string;
  completed: number | null;
  total: number | null;
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
  /** Skill-test questions only. */
  topic?: string;
  difficulty?: "easy" | "medium" | "hard";
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
  model: { question?: { provider: string; model: string }; track?: string; freeText?: boolean };
  questions: Question[];
  overall: Overall | null;
  topics: string[];
  createdAt: string;
  updatedAt: string;
}

export interface AttemptSummary {
  id: string;
  kind: "chapter" | "assessment";
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

export type StreamEvent =
  | { type: "token"; text: string }
  | { type: "status"; message: string }
  | { type: "speech"; text: string };

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

export interface SkillTrack {
  id: string;
  label: string;
  topicCount: number;
}

export type TopicStatus = "untested" | "weak" | "learning" | "strong";

export interface TopicView {
  id: string;
  label: string;
  description: string;
  prereqs: string[];
  layer: number;
  mastery: number | null;
  confidence: number;
  evidence: number;
  status: TopicStatus;
}

export interface SkillMap {
  track: string;
  label: string;
  topics: TopicView[];
  lastAssessment: {
    id: string;
    status: "in_progress" | "completed";
    score: number | null;
    summary: string | null;
    strengths: string[];
    focusNext: string[];
    createdAt: string;
  } | null;
}

export interface AssessmentStep {
  attempt: Attempt;
  questionId: string | null;
  topicsDone: number;
  topicsTotal: number;
  skipped: string[];
}

export interface BookRef {
  fingerprint: string;
  title: string;
  status: BookStatus;
}

export interface StepChapter {
  idx: number;
  id: string;
  title: string;
  startPage: number;
  endPage: number;
  done: boolean;
}

export interface RoadmapStep {
  topicId: string;
  label: string;
  mastery: number | null;
  status: TopicStatus;
  reason: string;
  book: BookRef | null;
  chapters: StepChapter[];
  alternatives: BookRef[];
}

export interface Week {
  number: number;
  starts: string;
  pages: number;
  items: WeekItem[];
}

export interface Roadmap {
  track: string;
  label: string;
  pace: number;
  steps: RoadmapStep[];
  weeks: Week[];
  learned: string[];
  needsIndex: BookRef[];
}

export interface Prefs {
  goalTrack: string | null;
  weeklyPace: number;
  dailyCards: number;
}

export interface Card {
  id: string;
  front: string;
  back: string;
  topicId: string | null;
  attemptId: string | null;
  bookFingerprint: string | null;
  bookTitle: string | null;
  page: number | null;
  due: string;
  stability: number;
  difficulty: number;
  reps: number;
  lapses: number;
  state: "new" | "learning" | "review" | "relearning";
  lastReview: string | null;
  createdAt: string;
  /** Wait after Again / Hard / Good / Easy. */
  preview: [string, string, string, string];
}

export interface NewCard {
  front: string;
  back: string;
  topicId?: string | null;
  bookFingerprint?: string | null;
  page?: number | null;
}

export interface DueQueue {
  cards: Card[];
  dueNow: number;
  dailyLimit: number;
}

export interface WeekItem {
  topicId: string;
  topicLabel: string;
  book: BookRef;
  chapter: StepChapter;
}

export interface Dashboard {
  cards: { dueNow: number; dueTomorrow: number; total: number; reviewedToday: number };
  streak: { current: number; longest: number; activeToday: boolean };
  activity: { day: string; quizzes: number; reviews: number; minutes: number }[];
  scoreTrend: { id: string; date: string; score: number; title: string }[];
  reading: { today: number; week: number; total: number };
  books: { fingerprint: string; title: string; lastPage: number; pageCount: number | null; chaptersDone: number; minutes: number }[];
  usage: { provider: string; model: string; calls: number; promptTokens: number; completionTokens: number }[];
  goalTrack: string;
  goalLabel: string;
  weakTopics: { id: string; label: string; mastery: number }[];
  untestedTopics: number;
  nextStep: WeekItem | null;
  dailyLimit: number;
}

export interface Persona {
  id: string;
  name: string;
  role: string;
  style: string;
  probing: string;
  hints: string;
  challenges: string;
}

export type InterviewSource = "chapter" | "book" | "topics" | "general";
export type Level = "junior" | "mid" | "senior";

export interface InterviewSetup {
  source: InterviewSource;
  bookFingerprint: string | null;
  chapterIdx: number | null;
  track: string | null;
  topicIds: string[];
  level: Level;
  minutes: number;
  personaId: string | null;
  /** "auto" is resolved when the interview starts. */
  mode: "auto" | "fast" | "thorough";
}

export interface ModelCheck {
  items: {
    role: string;
    job: string;
    providerId: string | null;
    model: string | null;
    level: "ok" | "warn" | "bad";
    messages: string[];
    tested: { jsonOk: boolean; latencyMs: number; at: string } | null;
  }[];
  recommendedMode: "fast" | "thorough";
  blocking: boolean;
}

export type PointStatus = "untested" | "verified" | "partial" | "missing";

export interface IQuestion {
  id: string;
  kind: "concept" | "scenario" | "design" | "code";
  difficulty: "easy" | "medium" | "hard";
  /** 1-based position once asked; spare questions that weren't needed stay null. */
  order: number | null;
  topic: string | null;
  prompt: string;
  code: string | null;
  reference: string;
  sources: Source[];
  points: { text: string; status: PointStatus; probes: number; angles: string[] }[];
  turns: number;
  hints: number;
  done: boolean;
  score: number | null;
}

export interface Turn {
  role: "interviewer" | "candidate";
  kind: string;
  text: string;
  question: number;
  at: string;
}

export interface InterviewReport {
  overall: number;
  verdict: "strong" | "good" | "needs_work";
  coverage: number;
  depth: number | null;
  clarity: number | null;
  summary: string;
  strengths: string[];
  gaps: string[];
  bestMoment: string | null;
  weakestMoment: string | null;
  notes: { question: number; note: string }[];
  rereadPages: number[];
}

export interface Interview {
  id: string;
  title: string;
  personaId: string;
  setup: InterviewSetup;
  questions: IQuestion[];
  transcript: Turn[];
  current: number;
  status: "in_progress" | "wrapping_up" | "completed";
  report: InterviewReport | null;
  topics: string[];
  createdAt: string;
  updatedAt: string;
}

export interface InterviewSummary {
  id: string;
  title: string;
  personaId: string;
  personaName: string;
  level: Level;
  status: Interview["status"];
  overall: number | null;
  verdict: InterviewReport["verdict"] | null;
  questions: number;
  asked: number;
  planned: number;
  createdAt: string;
}

export interface Term {
  term: string;
  definition: string;
  page: number | null;
}

export interface LessonTopic {
  title: string;
  pageStart: number;
  pageEnd: number;
  lesson: { plain: string; industry: string; recap: string; terms: Term[]; createdAt: string } | null;
}

export interface Lesson {
  id: string;
  fingerprint: string;
  chapterId: string;
  chapterIdx: number;
  chapterTitle: string;
  bookTitle: string;
  pageStart: number;
  pageEnd: number;
  overview: string;
  summary: string[];
  terms: Term[];
  topics: LessonTopic[];
  createdAt: string;
  updatedAt: string;
}

export interface LessonSummary {
  id: string;
  fingerprint: string;
  bookTitle: string;
  chapterIdx: number;
  chapterTitle: string;
  terms: number;
  topics: number;
  topicsTaught: number;
  updatedAt: string;
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

  systemInfo: () => invoke<SystemInfo>("system_info"),
  ollamaCatalog: (providerId: string) => invoke<CatalogView>("ollama_catalog", { providerId }),
  ollamaLookup: (name: string) => invoke<LookupResult>("ollama_lookup", { name }),
  ollamaPull: (requestId: string, providerId: string, name: string, onProgress: (p: PullProgress) => void) =>
    invoke<Provider>("ollama_pull", { requestId, providerId, name, onProgress: channel(onProgress) }),
  ollamaDelete: (providerId: string, name: string) => invoke<void>("ollama_delete", { providerId, name }),

  cancel: (requestId: string) => invoke<void>("ai_cancel", { requestId }),
  generateQuiz: (
    requestId: string,
    fingerprint: string,
    chapterIdx: number,
    options: QuizOptions,
    onEvent: (e: StreamEvent) => void,
  ) => invoke<Attempt>("quiz_generate", { requestId, fingerprint, chapterIdx, options, onEvent: channel(onEvent) }),
  saveAnswer: (attemptId: string, questionId: string, answer: string) =>
    invoke<Attempt>("quiz_save_answer", { attemptId, questionId, answer }),
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

  skillTracks: () => invoke<SkillTrack[]>("skill_tracks"),
  startAssessment: (track: string, freeText: boolean) => invoke<Attempt>("assessment_start", { track, freeText }),
  assessmentNext: (requestId: string, attemptId: string, onEvent: (e: StreamEvent) => void) =>
    invoke<AssessmentStep>("assessment_next", { requestId, attemptId, onEvent: channel(onEvent) }),
  assessmentPrefetch: (requestId: string, attemptId: string) =>
    invoke<void>("assessment_prefetch", { requestId, attemptId }),
  skills: (track: string) => invoke<SkillMap>("skills_get", { track }),
  roadmap: (track: string) => invoke<Roadmap>("roadmap_get", { track }),
  prepareRoadmap: (track: string, onProgress: (p: { done: number; total: number; title: string }) => void) =>
    invoke<string[]>("roadmap_prepare", { track, onProgress: channel(onProgress) }),
  prefs: () => invoke<Prefs>("prefs_get"),
  setPrefs: (p: { goalTrack?: string; weeklyPace?: number; dailyCards?: number }) =>
    invoke<Prefs>("prefs_set", {
      goalTrack: p.goalTrack ?? null,
      weeklyPace: p.weeklyPace ?? null,
      dailyCards: p.dailyCards ?? null,
    }),

  cardsDue: () => invoke<DueQueue>("cards_due"),
  reviewCard: (id: string, rating: 1 | 2 | 3 | 4) => invoke<Card>("card_review", { id, rating }),
  cards: (query?: string) => invoke<Card[]>("cards_list", { query: query ?? null }),
  createCard: (card: NewCard) => invoke<Card>("card_create", { card }),
  updateCard: (id: string, front: string, back: string) => invoke<Card>("card_update", { id, front, back }),
  deleteCard: (id: string) => invoke<void>("card_delete", { id }),
  readingTick: (fingerprint: string, minutes: number) => invoke<void>("reading_tick", { fingerprint, minutes }),
  dashboard: () => invoke<Dashboard>("dashboard_get"),

  personas: () => invoke<Persona[]>("interview_personas"),
  startInterview: (requestId: string, setup: InterviewSetup, onEvent: (e: StreamEvent) => void) =>
    invoke<Interview>("interview_start", { requestId, setup, onEvent: channel(onEvent) }),
  interviewSay: (id: string, kind: "answer" | "hint_request" | "dont_know", text: string) =>
    invoke<Interview>("interview_say", { id, kind, text }),
  interviewRespond: (requestId: string, id: string, onEvent: (e: StreamEvent) => void) =>
    invoke<Interview>("interview_respond", { requestId, id, onEvent: channel(onEvent) }),
  finishInterview: (requestId: string, id: string, onEvent: (e: StreamEvent) => void) =>
    invoke<Interview>("interview_finish", { requestId, id, onEvent: channel(onEvent) }),
  lesson: (fingerprint: string, chapterIdx: number) => invoke<Lesson | null>("lesson_get", { fingerprint, chapterIdx }),
  summarizeChapter: (requestId: string, fingerprint: string, chapterIdx: number, onEvent: (e: StreamEvent) => void) =>
    invoke<Lesson>("lesson_summarize", { requestId, fingerprint, chapterIdx, onEvent: channel(onEvent) }),
  teachTopic: (requestId: string, lessonId: string, topicIdx: number, onEvent: (e: StreamEvent) => void) =>
    invoke<Lesson>("lesson_teach", { requestId, lessonId, topicIdx, onEvent: channel(onEvent) }),
  lessons: () => invoke<LessonSummary[]>("lessons_list"),
  deleteLesson: (id: string) => invoke<void>("lesson_delete", { id }),
  termsToCards: (lessonId: string, terms: string[]) => invoke<number>("lesson_terms_to_cards", { lessonId, terms }),
  interviewCheck: (minutes: number) => invoke<ModelCheck>("interview_check", { minutes }),
  interviews: () => invoke<InterviewSummary[]>("interviews_list"),
  interview: (id: string) => invoke<Interview>("interview_get", { id }),
  deleteInterview: (id: string) => invoke<void>("interview_delete", { id }),

  exportProgress: (path: string) => invoke<void>("export_progress", { path }),
  importPreview: (path: string) => invoke<ImportPreview>("import_preview", { path }),
  importApply: (path: string, mode: "merge" | "replace") => invoke<TableChanges[]>("import_apply", { path, mode }),
};
