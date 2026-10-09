import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate, useParams, useSearchParams } from "react-router";
import {
  ArrowLeft,
  BookOpenText,
  CheckCircle2,
  GraduationCap,
  Layers,
  Minus,
  MoveHorizontal,
  Plus,
  Search,
  X,
} from "lucide-react";
import type { PDFViewer as PDFViewerType } from "pdfjs-dist/web/pdf_viewer.mjs";
import type { PDFDocumentProxy } from "pdfjs-dist";
import { api, errorMessage, newRequestId, type Chapter, type QuizOptions } from "../ipc";
import { pdfjsLib, viewerModule } from "../pdf";
import { useBooks } from "./Library";
import { StreamBox } from "../components/StreamBox";
import { CardEditor } from "./Cards";
import { Badge, Button, ErrorNote, Field, inputClass, Modal, Spinner } from "../ui";

function useIndexedChapters(fingerprint: string) {
  const qc = useQueryClient();
  const [progress, setProgress] = useState<{ page: number; total: number } | null>(null);
  const status = useQuery({
    queryKey: ["index", fingerprint],
    queryFn: async () => {
      const st = await api.indexStatus(fingerprint);
      if (st.indexed) return st;
      const res = await api.indexBook(fingerprint, false, setProgress);
      qc.invalidateQueries({ queryKey: ["books"] });
      return res;
    },
    staleTime: Infinity,
  });
  const chapters = useQuery({
    queryKey: ["chapters", fingerprint],
    queryFn: () => api.bookChapters(fingerprint),
    enabled: !!status.data?.indexed,
    staleTime: Infinity,
  });
  return { status, chapters, progress };
}

function QuizSetup({
  fingerprint,
  chapter,
  onClose,
}: {
  fingerprint: string;
  chapter: Chapter;
  onClose: () => void;
}) {
  const navigate = useNavigate();
  const [opts, setOpts] = useState<QuizOptions>({ count: 5, difficulty: "medium", types: ["mcq", "short", "explain"] });
  const [running, setRunning] = useState(false);
  const [status, setStatus] = useState("");
  const [text, setText] = useState("");
  const [error, setError] = useState<string | null>(null);
  const requestId = useRef(newRequestId());

  async function start() {
    setRunning(true);
    setError(null);
    setText("");
    requestId.current = newRequestId();
    try {
      const attempt = await api.generateQuiz(requestId.current, fingerprint, chapter.idx, opts, (e) => {
        if (e.type === "status") setStatus(e.message);
        else setText((t) => t + e.text);
      });
      navigate(`/quiz/${attempt.id}`);
    } catch (e) {
      setError(errorMessage(e));
      setRunning(false);
    }
  }

  function close() {
    if (running) api.cancel(requestId.current);
    onClose();
  }

  const toggleType = (t: "mcq" | "short" | "explain") =>
    setOpts((o) => ({
      ...o,
      types: o.types.includes(t) ? (o.types.length > 1 ? o.types.filter((x) => x !== t) : o.types) : [...o.types, t],
    }));

  return (
    <Modal
      title="Quiz me on this chapter"
      onClose={close}
      footer={
        <>
          <Button onClick={close}>{running ? "Cancel" : "Close"}</Button>
          {!running && (
            <Button variant="primary" onClick={start}>
              Start quiz
            </Button>
          )}
        </>
      }
    >
      <p className="mb-4 text-sm">
        <span className="font-medium">{chapter.title}</span>{" "}
        <span className="text-muted">
          · pages {chapter.startPage}–{chapter.endPage}
        </span>
      </p>
      {running ? (
        <StreamBox status={status} text={text} />
      ) : (
        <>
          <div className="grid grid-cols-2 gap-3">
            <Field label="Questions">
              <select
                className={inputClass}
                value={opts.count}
                onChange={(e) => setOpts({ ...opts, count: Number(e.target.value) })}
              >
                {[3, 5, 8, 10, 15].map((n) => (
                  <option key={n}>{n}</option>
                ))}
              </select>
            </Field>
            <Field label="Difficulty">
              <select
                className={inputClass}
                value={opts.difficulty}
                onChange={(e) => setOpts({ ...opts, difficulty: e.target.value as QuizOptions["difficulty"] })}
              >
                <option value="easy">Easy — main ideas</option>
                <option value="medium">Medium — applying concepts</option>
                <option value="hard">Hard — edge cases & trade-offs</option>
              </select>
            </Field>
          </div>
          <Field label="Question types">
            <div className="flex gap-3 text-sm">
              {(
                [
                  ["mcq", "Multiple choice"],
                  ["short", "Short answer"],
                  ["explain", "Explain in your words"],
                ] as const
              ).map(([t, label]) => (
                <label key={t} className="flex items-center gap-1.5">
                  <input type="checkbox" checked={opts.types.includes(t)} onChange={() => toggleType(t)} />
                  {label}
                </label>
              ))}
            </div>
          </Field>
          <p className="text-xs text-muted">
            Questions are written from this chapter's text and cite the pages they come from. On a local model this can
            take a minute or two.
          </p>
        </>
      )}
      <div className="mt-3">
        <ErrorNote error={error} />
      </div>
    </Modal>
  );
}

export default function ReaderPage() {
  const { fingerprint = "" } = useParams();
  const [params] = useSearchParams();
  const navigate = useNavigate();
  const qc = useQueryClient();
  const books = useBooks();
  const book = books.data?.find((b) => b.fingerprint === fingerprint);
  const { status, chapters, progress } = useIndexedChapters(fingerprint);

  const containerRef = useRef<HTMLDivElement>(null);
  const viewerRef = useRef<PDFViewerType | null>(null);
  // Typical page width in PDF points (not page 1: covers are often smaller than the rest).
  const typicalWidth = useRef<number | null>(null);
  // Follow the window width until the reader zooms by hand.
  const fitMode = useRef(true);
  const [page, setPage] = useState(1);
  const [pageCount, setPageCount] = useState(0);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [quizChapter, setQuizChapter] = useState<Chapter | null>(null);
  const [searchQ, setSearchQ] = useState("");
  const [searchOpen, setSearchOpen] = useState(false);
  const [cardDraft, setCardDraft] = useState<{ back: string; page: number } | null>(null);

  // Reading time: one minute per minute the window is visible and you've scrolled/moved/typed recently.
  useEffect(() => {
    let lastActive = Date.now();
    const mark = () => {
      lastActive = Date.now();
    };
    const events = ["scroll", "wheel", "mousemove", "keydown"] as const;
    events.forEach((e) => window.addEventListener(e, mark, { capture: true, passive: true }));
    const timer = setInterval(() => {
      if (document.visibilityState === "visible" && Date.now() - lastActive < 120_000) {
        api.readingTick(fingerprint, 1).catch(() => {});
      }
    }, 60_000);
    return () => {
      clearInterval(timer);
      events.forEach((e) => window.removeEventListener(e, mark, { capture: true }));
    };
  }, [fingerprint]);

  const saveProgress = useMutation({
    mutationFn: (p: { lastPage?: number; chapterDone?: string; chapterUndone?: string }) => api.saveProgress(fingerprint, p),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["books"] }),
  });

  // Load the PDF once the book row is known
  const initialPage = useRef<number | null>(null);
  if (initialPage.current === null && book) {
    initialPage.current = Number(params.get("page")) || book.progress?.lastPage || 1;
  }
  const bookKnown = !!book;

  useEffect(() => {
    if (!bookKnown || !containerRef.current) return;
    let doc: PDFDocumentProxy | null = null;
    let cancelled = false;
    (async () => {
      try {
        const { EventBus, PDFViewer, PDFLinkService } = await viewerModule;
        const bytes = await api.readBookFile(fingerprint);
        if (cancelled) return;
        doc = await pdfjsLib.getDocument({ data: new Uint8Array(bytes) }).promise;
        if (cancelled) return;
        const eventBus = new EventBus();
        const linkService = new PDFLinkService({ eventBus });
        const container = containerRef.current!;
        const viewer = new PDFViewer({ container, eventBus, linkService, removePageBorders: true });
        linkService.setViewer(viewer);
        viewerRef.current = viewer;
        eventBus.on("pagesinit", async () => {
          const d = doc!;
          const n = d.numPages;
          const sample = [...new Set([2, 3, 4, 5, Math.floor(n / 3), Math.floor(n / 2), n - 1])].filter((p) => p >= 1 && p <= n);
          try {
            const widths = await Promise.all(
              sample.map(async (p) => (await d.getPage(p)).getViewport({ scale: 1 }).width),
            );
            widths.sort((a, b) => a - b);
            typicalWidth.current = widths[Math.floor(widths.length / 2)] || null;
          } catch {
            typicalWidth.current = null;
          }
          if (cancelled) return;
          fitWidth();
          viewer.currentPageNumber = Math.min(initialPage.current ?? 1, n);
          setLoading(false);
        });
        eventBus.on("pagechanging", (e: { pageNumber: number }) => setPage(e.pageNumber));
        viewer.setDocument(doc);
        linkService.setDocument(doc);
        setPageCount(doc.numPages);
      } catch (e) {
        setLoadError(errorMessage(e));
        setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
      viewerRef.current = null;
      doc?.destroy();
    };
  }, [fingerprint, bookKnown]);

  // Save the reading position a moment after the page settles
  const lastSaved = useRef(0);
  useEffect(() => {
    if (loading || page === lastSaved.current) return;
    const t = setTimeout(() => {
      lastSaved.current = page;
      saveProgress.mutate({ lastPage: page });
    }, 1500);
    return () => clearTimeout(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [page, loading]);

  const goTo = useCallback((p: number) => {
    if (viewerRef.current) viewerRef.current.currentPageNumber = p;
  }, []);

  /** Scale so a typical page fills the width (falls back to pdf.js's own fit if sizes are unknown). */
  function fitWidth() {
    const v = viewerRef.current;
    const container = containerRef.current;
    if (!v || !container) return;
    fitMode.current = true;
    const w = typicalWidth.current;
    if (!w) {
      v.currentScaleValue = "page-width";
      return;
    }
    const cssWidth = w * (96 / 72); // PDF points → CSS pixels at scale 1
    v.currentScale = Math.min(4, Math.max(0.25, (container.clientWidth - 40) / cssWidth));
  }

  // Keep fitting the width when the window (or the search panel) changes size.
  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    let t: ReturnType<typeof setTimeout> | undefined;
    const ro = new ResizeObserver(() => {
      clearTimeout(t);
      t = setTimeout(() => fitMode.current && fitWidth(), 150);
    });
    ro.observe(container);
    return () => {
      clearTimeout(t);
      ro.disconnect();
    };
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  const zoom = (dir: 1 | -1) => {
    const v = viewerRef.current;
    fitMode.current = false;
    if (v) v.currentScale = Math.min(4, Math.max(0.3, v.currentScale * (dir > 0 ? 1.15 : 1 / 1.15)));
  };

  const currentChapter = useMemo(
    () => chapters.data?.find((c) => page >= c.startPage && page <= c.endPage) ?? null,
    [chapters.data, page],
  );

  const search = useQuery({
    queryKey: ["search", fingerprint, searchQ],
    queryFn: () => api.searchBook(fingerprint, searchQ),
    enabled: searchOpen && searchQ.trim().length >= 2 && !!status.data?.indexed,
  });

  // Arriving from a lesson's "Quiz me": open the quiz for the chapter of the requested page.
  const quizParam = useRef(params.get("quiz") === "1");
  useEffect(() => {
    if (!quizParam.current || !chapters.data) return;
    const p = Number(params.get("page")) || 1;
    const c = chapters.data.find((x) => p >= x.startPage && p <= x.endPage);
    if (c) {
      quizParam.current = false;
      setQuizChapter(c);
    }
  }, [chapters.data]); // eslint-disable-line react-hooks/exhaustive-deps

  const done = new Set(book?.progress?.chaptersDone ?? []);
  const scanned = status.data?.indexed && status.data.textPages < status.data.pageCount * 0.1;

  return (
    <div className="flex h-full flex-col overflow-clip">
      <header className="flex items-center gap-2 border-b border-line bg-panel px-3 py-2">
        <Button variant="ghost" onClick={() => navigate("/library")} aria-label="Back to library">
          <ArrowLeft className="size-4" />
        </Button>
        <div className="min-w-0">
          <div className="truncate text-sm font-semibold">{book?.title ?? "…"}</div>
          <div className="truncate text-xs text-muted">{currentChapter?.title ?? " "}</div>
        </div>
        <div className="ml-auto flex items-center gap-1">
          <span className="mr-2 text-xs text-muted tabular-nums">
            {page} / {pageCount || "…"}
          </span>
          <Button variant="ghost" onClick={() => zoom(-1)} aria-label="Zoom out">
            <Minus className="size-4" />
          </Button>
          <Button variant="ghost" onClick={() => zoom(1)} aria-label="Zoom in">
            <Plus className="size-4" />
          </Button>
          <Button variant="ghost" onClick={fitWidth} aria-label="Fit width" title="Fit width">
            <MoveHorizontal className="size-4" />
          </Button>
          <Button variant="ghost" onClick={() => setSearchOpen((o) => !o)} aria-label="Search in book">
            <Search className="size-4" />
          </Button>
          <Button
            variant="ghost"
            title="Make a review card (select text in the page first to use it as the answer)"
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => setCardDraft({ back: window.getSelection()?.toString().trim() ?? "", page })}
          >
            <Layers className="size-4" /> Card
          </Button>
          <Button
            variant="ghost"
            disabled={!currentChapter || !!scanned}
            onClick={() => currentChapter && navigate(`/lesson/${fingerprint}/${currentChapter.idx}`)}
            title="Summary with every keyword, then the chapter taught topic by topic"
          >
            <BookOpenText className="size-4" /> Teach me
          </Button>
          <Button
            variant="primary"
            disabled={!currentChapter || !!scanned}
            onClick={() => currentChapter && setQuizChapter(currentChapter)}
            title={scanned ? "This book is scanned images — no text to quiz on" : "Quiz yourself on the current chapter"}
          >
            <GraduationCap className="size-4" /> Quiz me
          </Button>
        </div>
      </header>

      <div className="flex min-h-0 flex-1 overflow-clip">
        <aside className="w-64 shrink-0 overflow-x-hidden overflow-y-auto border-r border-line bg-panel">
          <div className="px-3 pt-3 pb-1 text-xs font-semibold tracking-wide text-muted uppercase">Chapters</div>
          {status.isLoading && (
            <div className="px-3 py-2 text-xs text-muted">
              <div className="mb-1 flex items-center gap-2">
                <Spinner /> Preparing book for quizzes…
              </div>
              {progress && (
                <div className="h-1 overflow-hidden rounded bg-sunken">
                  <div className="h-full bg-accent" style={{ width: `${(progress.page / progress.total) * 100}%` }} />
                </div>
              )}
            </div>
          )}
          {status.error && (
            <div className="p-3">
              <ErrorNote error={errorMessage(status.error)} />
            </div>
          )}
          {scanned && (
            <div className="px-3 py-2 text-xs text-warn">
              This PDF is scanned images with no text layer, so quizzes aren't available for it.
            </div>
          )}
          <ul className="pb-4">
            {chapters.data?.map((c) => {
              const active = currentChapter?.idx === c.idx;
              return (
                <li key={c.idx}>
                  <button
                    className={`group flex w-full items-start gap-1.5 px-3 py-1 text-left text-[13px] ${active ? "bg-accent-soft text-accent" : "hover:bg-sunken"}`}
                    onClick={() => goTo(c.startPage)}
                  >
                    <CheckCircle2
                      className={`mt-0.5 size-3.5 shrink-0 ${done.has(c.id) ? "text-good" : "text-transparent"}`}
                    />
                    <span className="min-w-0 flex-1">{c.title}</span>
                    <span className="shrink-0 text-[11px] text-muted tabular-nums">{c.startPage}</span>
                  </button>
                </li>
              );
            })}
          </ul>
        </aside>

        <div className="relative min-w-0 flex-1 overflow-clip bg-sunken">
          <div ref={containerRef} className="pdf-scroll">
            <div className="pdfViewer" />
          </div>
          {loading && !loadError && (
            <div className="absolute inset-0 flex items-center justify-center">
              <Spinner />
            </div>
          )}
          {loadError && (
            <div className="absolute inset-x-0 top-0 p-6">
              <ErrorNote error={loadError} />
            </div>
          )}
        </div>

        {searchOpen && (
          <aside className="flex w-80 shrink-0 flex-col border-l border-line bg-panel">
            <div className="flex items-center gap-2 border-b border-line p-2">
              <input
                autoFocus
                className={inputClass}
                placeholder="Search this book"
                value={searchQ}
                onChange={(e) => setSearchQ(e.target.value)}
              />
              <Button variant="ghost" onClick={() => setSearchOpen(false)} aria-label="Close search">
                <X className="size-4" />
              </Button>
            </div>
            <div className="flex-1 overflow-y-auto">
              {search.isFetching && <Spinner className="m-3" />}
              {search.data?.length === 0 && <p className="p-3 text-xs text-muted">No matches.</p>}
              {search.data?.map((h) => (
                <button
                  key={h.chunkId}
                  className="block w-full border-b border-line px-3 py-2 text-left text-xs hover:bg-sunken"
                  onClick={() => goTo(h.pageStart)}
                >
                  <Badge>
                    p. {h.pageStart}
                    {h.pageEnd !== h.pageStart ? `–${h.pageEnd}` : ""}
                  </Badge>
                  <p className="mt-1 text-muted">
                    {h.snippet.split(/[«»]/).map((part, i) =>
                      i % 2 ? (
                        <mark key={i} className="bg-accent-soft text-ink">
                          {part}
                        </mark>
                      ) : (
                        part
                      ),
                    )}
                  </p>
                </button>
              ))}
            </div>
          </aside>
        )}
      </div>

      {cardDraft && (
        <CardEditor
          initial={{ front: "", back: cardDraft.back, bookFingerprint: fingerprint, page: cardDraft.page }}
          onClose={() => setCardDraft(null)}
        />
      )}
      {quizChapter && <QuizSetup fingerprint={fingerprint} chapter={quizChapter} onClose={() => setQuizChapter(null)} />}
    </div>
  );
}
