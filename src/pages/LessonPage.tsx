import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate, useParams } from "react-router";
import {
  ArrowLeft,
  BookOpen,
  Briefcase,
  Check,
  GraduationCap,
  Lightbulb,
  ListChecks,
  Plus,
  RotateCcw,
  Search,
} from "lucide-react";
import { api, errorMessage, newRequestId, type Lesson, type StreamEvent, type Term } from "../ipc";
import { Md } from "./Quiz";
import { useBooks } from "./Library";
import { StreamBox } from "../components/StreamBox";
import { Badge, Button, ErrorNote, inputClass, Spinner } from "../ui";

function PageLink({ fp, page }: { fp: string; page: number | null }) {
  if (!page) return null;
  return (
    <Link
      to={`/book/${fp}?page=${page}`}
      className="inline-flex shrink-0 items-center gap-0.5 text-[11px] text-accent hover:underline"
    >
      <BookOpen className="size-3" /> p. {page}
    </Link>
  );
}

/** Live progress for a model call: status line + what it is writing. */
function useStream() {
  const [status, setStatus] = useState("");
  const [text, setText] = useState("");
  const onEvent = (e: StreamEvent) => {
    if (e.type === "status") {
      setStatus(e.message);
      setText("");
    } else setText((t) => t + e.text);
  };
  const reset = () => {
    setStatus("");
    setText("");
  };
  return { status, text, onEvent, reset };
}

function Glossary({ lesson }: { lesson: Lesson }) {
  const qc = useQueryClient();
  const [filter, setFilter] = useState("");
  const [added, setAdded] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const shown = lesson.terms.filter(
    (t) => !filter.trim() || `${t.term} ${t.definition}`.toLowerCase().includes(filter.trim().toLowerCase()),
  );

  async function add(terms: Term[]) {
    setBusy(true);
    setError(null);
    try {
      const n = await api.termsToCards(
        lesson.id,
        terms.map((t) => t.term),
      );
      setAdded((s) => new Set([...s, ...terms.map((t) => t.term.toLowerCase())]));
      setNote(n === 0 ? "Those are already cards." : `${n} new card${n === 1 ? "" : "s"} — due now in Cards.`);
      qc.invalidateQueries({ queryKey: ["cards"] });
      qc.invalidateQueries({ queryKey: ["dashboard"] });
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="rounded-lg border border-line bg-panel p-5">
      <div className="mb-3 flex flex-wrap items-center gap-3">
        <h2 className="text-base font-semibold">Keywords</h2>
        <Badge>{lesson.terms.length}</Badge>
        <div className="relative ml-auto w-56">
          <Search className="absolute top-2 left-2.5 size-4 text-muted" />
          <input
            className={`${inputClass} pl-8`}
            placeholder="Filter…"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
          />
        </div>
        <Button
          busy={busy}
          onClick={() => add(lesson.terms)}
          title="Each term becomes a review card: “What is …?” → definition"
        >
          <Plus className="size-4" /> Add all to cards
        </Button>
      </div>
      {note && <p className="mb-2 text-xs text-good">{note}</p>}
      <ErrorNote error={error} />
      <dl className="grid gap-x-6 gap-y-3 md:grid-cols-2">
        {shown.map((t) => {
          const isAdded = added.has(t.term.toLowerCase());
          return (
            <div key={t.term} className="group">
              <dt className="flex items-center gap-2 text-sm font-semibold">
                {t.term}
                <PageLink fp={lesson.fingerprint} page={t.page} />
                <button
                  className={`ml-auto rounded p-0.5 ${isAdded ? "text-good" : "text-muted opacity-0 group-hover:opacity-100 hover:bg-sunken"}`}
                  disabled={isAdded || busy}
                  onClick={() => add([t])}
                  title={isAdded ? "Added to cards" : "Add to cards"}
                  aria-label="Add to cards"
                >
                  {isAdded ? <Check className="size-3.5" /> : <Plus className="size-3.5" />}
                </button>
              </dt>
              <dd className="text-sm text-muted">{t.definition}</dd>
            </div>
          );
        })}
      </dl>
    </section>
  );
}

function SummaryStep({ lesson, onNext }: { lesson: Lesson; onNext: () => void }) {
  return (
    <div className="flex flex-col gap-4">
      <section className="rounded-lg border border-line bg-panel p-5">
        <div className="mb-2 text-base leading-relaxed">
          <Md>{lesson.overview}</Md>
        </div>
        <ul className="flex list-disc flex-col gap-1.5 pl-5 text-sm leading-relaxed">
          {lesson.summary.map((b, i) => (
            <li key={i}>
              <Md>{b}</Md>
            </li>
          ))}
        </ul>
      </section>
      <Glossary lesson={lesson} />
      <div className="flex justify-end">
        <Button variant="primary" onClick={onNext}>
          Continue to the lesson by topic →
        </Button>
      </div>
    </div>
  );
}

function Layer({ icon, title, children }: { icon: React.ReactNode; title: string; children: string }) {
  return (
    <div className="mt-4">
      <h4 className="mb-1 flex items-center gap-1.5 text-xs font-semibold tracking-wide text-muted uppercase">
        {icon} {title}
      </h4>
      <div className="text-sm leading-relaxed">
        <Md>{children}</Md>
      </div>
    </div>
  );
}

function LessonStep({ lesson, setLesson }: { lesson: Lesson; setLesson: (l: Lesson) => void }) {
  const stream = useStream();
  const [teaching, setTeaching] = useState<number | null>(null);
  const [all, setAll] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const request = useRef<string | null>(null);
  const stopAll = useRef(false);

  useEffect(() => () => void (request.current && api.cancel(request.current)), []);

  async function teach(i: number, current: Lesson): Promise<Lesson | null> {
    setTeaching(i);
    setError(null);
    stream.reset();
    try {
      request.current = newRequestId();
      const l = await api.teachTopic(request.current, current.id, i, stream.onEvent);
      setLesson(l);
      return l;
    } catch (e) {
      setError(errorMessage(e));
      return null;
    } finally {
      setTeaching(null);
      request.current = null;
    }
  }

  async function teachAll() {
    setAll(true);
    stopAll.current = false;
    let current: Lesson | null = lesson;
    for (let i = 0; i < lesson.topics.length && current && !stopAll.current; i++) {
      if (!current.topics[i].lesson) current = await teach(i, current);
    }
    setAll(false);
  }

  const remaining = lesson.topics.filter((t) => !t.lesson).length;

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center gap-3">
        <p className="flex-1 text-sm text-muted">
          Each topic is taught three ways: in plain words, the way engineers say it, and a short recap.
        </p>
        {remaining > 0 &&
          (all ? (
            <Button
              onClick={() => {
                stopAll.current = true;
                if (request.current) api.cancel(request.current);
              }}
            >
              Stop
            </Button>
          ) : (
            <Button variant="primary" disabled={teaching != null} onClick={teachAll}>
              <GraduationCap className="size-4" /> Teach all {remaining} topic{remaining === 1 ? "" : "s"}
            </Button>
          ))}
      </div>
      <ErrorNote error={error} />
      {lesson.topics.map((t, i) => (
        <section key={i} className="rounded-lg border border-line bg-panel p-5">
          <div className="flex items-center gap-2">
            <span className="text-sm font-semibold text-muted">{i + 1}.</span>
            <h3 className="flex-1 text-base font-semibold">{t.title}</h3>
            <Link
              to={`/book/${lesson.fingerprint}?page=${t.pageStart}`}
              className="text-xs text-accent hover:underline"
            >
              pp. {t.pageStart}–{t.pageEnd}
            </Link>
            {t.lesson && teaching !== i && (
              <Button
                variant="ghost"
                className="text-xs"
                disabled={teaching != null}
                onClick={() => teach(i, lesson)}
                title="Teach this topic again"
              >
                <RotateCcw className="size-3.5" />
              </Button>
            )}
          </div>
          {teaching === i ? (
            <div className="mt-3">
              <StreamBox status={stream.status} text={stream.text} />
            </div>
          ) : t.lesson ? (
            <>
              <Layer icon={<Lightbulb className="size-3.5" />} title="In plain words">
                {t.lesson.plain}
              </Layer>
              <Layer icon={<Briefcase className="size-3.5" />} title="In industry terms">
                {t.lesson.industry}
              </Layer>
              <Layer icon={<ListChecks className="size-3.5" />} title="In short">
                {t.lesson.recap}
              </Layer>
              {t.lesson.terms.length > 0 && (
                <div className="mt-3 flex flex-wrap gap-1.5">
                  {t.lesson.terms.map((x) => (
                    <span
                      key={x.term}
                      title={x.definition}
                      className="rounded-full bg-accent-soft px-2 py-0.5 text-[11px] font-medium text-accent"
                    >
                      {x.term}
                    </span>
                  ))}
                </div>
              )}
            </>
          ) : (
            <Button className="mt-3" disabled={teaching != null} onClick={() => teach(i, lesson)}>
              Teach me this
            </Button>
          )}
        </section>
      ))}
      <section className="flex flex-wrap items-center gap-3 rounded-lg border border-line bg-panel p-5">
        <span className="flex-1 text-sm">Check what stuck — the quiz uses the same chapter text.</span>
        <Link to={`/book/${lesson.fingerprint}?page=${lesson.pageStart}&quiz=1`}>
          <Button variant="primary">
            <GraduationCap className="size-4" /> Quiz me on this chapter
          </Button>
        </Link>
      </section>
    </div>
  );
}

export default function LessonPage() {
  const { fingerprint = "", chapterIdx = "0" } = useParams();
  const idx = Number(chapterIdx);
  const navigate = useNavigate();
  const qc = useQueryClient();
  const books = useBooks();
  const book = books.data?.find((b) => b.fingerprint === fingerprint);
  const chapters = useQuery({ queryKey: ["chapters", fingerprint], queryFn: () => api.bookChapters(fingerprint) });
  const chapter = chapters.data?.find((c) => c.idx === idx);
  const lesson = useQuery({ queryKey: ["lesson", fingerprint, idx], queryFn: () => api.lesson(fingerprint, idx) });
  const [step, setStep] = useState<1 | 2>(1);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const stream = useStream();
  const request = useRef<string | null>(null);

  useEffect(() => () => void (request.current && api.cancel(request.current)), []);

  const setLesson = (l: Lesson) => {
    qc.setQueryData(["lesson", fingerprint, idx], l);
    qc.invalidateQueries({ queryKey: ["lessons"] });
  };

  async function summarize() {
    setBusy(true);
    setError(null);
    stream.reset();
    try {
      request.current = newRequestId();
      setLesson(await api.summarizeChapter(request.current, fingerprint, idx, stream.onEvent));
      setStep(1);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
      request.current = null;
    }
  }

  const l = lesson.data;
  const title = l?.chapterTitle ?? chapter?.title ?? "Chapter";
  const prev = chapters.data?.find((c) => c.idx === idx - 1);
  const next = chapters.data?.find((c) => c.idx === idx + 1);

  return (
    <div className="h-full overflow-y-auto">
      <header className="sticky top-0 z-10 flex flex-wrap items-center gap-2 border-b border-line bg-panel px-3 py-2">
        <Button
          variant="ghost"
          onClick={() => navigate(`/book/${fingerprint}?page=${l?.pageStart ?? chapter?.startPage ?? 1}`)}
          aria-label="Back to the book"
        >
          <ArrowLeft className="size-4" />
        </Button>
        <div className="min-w-0">
          <div className="truncate text-sm font-semibold">{title}</div>
          <div className="truncate text-xs text-muted">{book?.title}</div>
        </div>
        {l && (
          <div className="ml-auto flex gap-1 rounded-lg bg-sunken p-1">
            {([1, 2] as const).map((n) => (
              <button
                key={n}
                onClick={() => setStep(n)}
                className={`rounded-md px-3 py-1 text-sm ${n === step ? "bg-panel font-medium shadow-sm" : "text-muted hover:text-ink"}`}
              >
                {n === 1 ? "1 · Summary & keywords" : "2 · Lesson by topic"}
              </button>
            ))}
          </div>
        )}
      </header>

      <div className="mx-auto max-w-3xl px-6 py-6">
        {(lesson.isLoading || chapters.isLoading) && <Spinner />}
        <ErrorNote error={lesson.error ? errorMessage(lesson.error) : error} />

        {busy && (
          <section className="mt-3 rounded-lg border border-line bg-panel p-5">
            <StreamBox status={stream.status || "Reading the chapter…"} text={stream.text} />
            <p className="mt-2 text-xs text-muted">
              Long chapters are read in parts. On a local model this takes a few minutes — it's saved, so only once.
            </p>
          </section>
        )}

        {!busy && lesson.isSuccess && !l && (
          <section className="rounded-lg border border-line bg-panel p-6">
            <h2 className="mb-1 text-lg font-semibold">{title}</h2>
            {chapter && (
              <p className="mb-4 text-sm text-muted">
                Pages {chapter.startPage}–{chapter.endPage} · {chapter.endPage - chapter.startPage + 1} pages
              </p>
            )}
            <p className="mb-4 text-sm">
              Your teacher reads the chapter and gives you <b>a concise summary with every keyword</b>, then{" "}
              <b>a lesson topic by topic</b>: in plain words, in industry terms, and a short recap.
            </p>
            <Button variant="primary" disabled={!chapter} onClick={summarize}>
              <GraduationCap className="size-4" /> Summarize this chapter
            </Button>
            {chapters.isSuccess && !chapter && (
              <p className="mt-2 text-xs text-muted">Open the book once so the app can read its chapters.</p>
            )}
          </section>
        )}

        {!busy && l && (
          <>
            {step === 1 ? (
              <SummaryStep lesson={l} onNext={() => setStep(2)} />
            ) : (
              <LessonStep lesson={l} setLesson={setLesson} />
            )}
            <div className="mt-6 flex flex-wrap items-center gap-3 border-t border-line pt-4 text-sm">
              {prev && (
                <Link to={`/lesson/${fingerprint}/${prev.idx}`} className="text-accent hover:underline">
                  ← {prev.title}
                </Link>
              )}
              <Button
                variant="ghost"
                className="mx-auto text-xs"
                onClick={summarize}
                title="Read the chapter again and rewrite the summary"
              >
                <RotateCcw className="size-3.5" /> Summarize again
              </Button>
              {next && (
                <Link to={`/lesson/${fingerprint}/${next.idx}`} className="text-accent hover:underline">
                  {next.title} →
                </Link>
              )}
            </div>
          </>
        )}
      </div>
    </div>
  );
}
