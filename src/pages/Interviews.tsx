import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "react-router";
import { AlertTriangle, CheckCircle2, Shuffle, XCircle } from "lucide-react";
import { api, errorMessage, newRequestId, type InterviewSetup, type InterviewSource, type Level } from "../ipc";
import { useBooks } from "./Library";
import { useGoalTrack } from "./Skills";
import { StreamBox } from "../components/StreamBox";
import { Badge, Button, Empty, ErrorNote, Field, formatDate, inputClass, Spinner } from "../ui";

export const verdictLabel = { strong: "Strong", good: "Good", needs_work: "Needs work" } as const;
export const verdictTone = { strong: "good", good: "warn", needs_work: "bad" } as const;

const SOURCES: { id: InterviewSource; label: string; hint: string }[] = [
  { id: "chapter", label: "A chapter", hint: "Questions from one chapter you've read" },
  { id: "book", label: "A book", hint: "Across the chapters you've finished (or the whole book)" },
  { id: "topics", label: "My weak topics", hint: "From your skill map, grounded in your books where possible" },
  { id: "general", label: "General", hint: "Like a real interview: not tied to your books" },
];

function Segmented<T extends string | number>({
  value,
  options,
  onChange,
}: {
  value: T;
  options: { id: T; label: string }[];
  onChange: (v: T) => void;
}) {
  return (
    <div className="inline-flex gap-1 rounded-lg bg-sunken p-1">
      {options.map((o) => (
        <button
          key={o.id}
          type="button"
          onClick={() => onChange(o.id)}
          className={`rounded-md px-3 py-1 text-sm ${o.id === value ? "bg-panel font-medium shadow-sm" : "text-muted hover:text-ink"}`}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

export function Avatar({ name, size = "size-9" }: { name: string; size?: string }) {
  const initials = name
    .replace(/^Dr\.\s*/, "")
    .split(/\s+/)
    .map((w) => w[0])
    .slice(0, 2)
    .join("");
  return (
    <span
      className={`inline-flex shrink-0 items-center justify-center rounded-full bg-accent-soft text-sm font-semibold text-accent ${size}`}
    >
      {initials}
    </span>
  );
}

const levelIcon = {
  ok: <CheckCircle2 className="size-4 text-good" />,
  warn: <AlertTriangle className="size-4 text-warn" />,
  bad: <XCircle className="size-4 text-bad" />,
};

/** Are the models this interview will use up to it? Checked before starting. */
function ModelCheckPanel({ minutes }: { minutes: number }) {
  const qc = useQueryClient();
  const check = useQuery({ queryKey: ["interviewCheck", minutes], queryFn: () => api.interviewCheck(minutes) });
  const [testing, setTesting] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function test(providerId: string, model: string) {
    setTesting(`${providerId}/${model}`);
    setError(null);
    try {
      await api.testModel(providerId, model);
      await qc.invalidateQueries({ queryKey: ["interviewCheck"] });
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setTesting(null);
    }
  }

  if (check.isLoading) return <Spinner />;
  if (check.error) return <ErrorNote error={errorMessage(check.error)} />;
  return (
    <div className="flex flex-col gap-1.5">
      {check.data?.items.map((it) => (
        <div key={it.role} className="flex items-start gap-2 text-sm">
          <span className="mt-0.5">{levelIcon[it.level]}</span>
          <div className="min-w-0 flex-1">
            <span className="font-medium">{it.job}</span>
            {it.model && <span className="ml-1.5 font-mono text-xs text-muted">{it.model}</span>}
            {it.tested && (
              <span className="ml-1.5 text-xs text-muted">· last test {(it.tested.latencyMs / 1000).toFixed(1)} s</span>
            )}
            {it.messages.map((m, i) => (
              <p key={i} className="text-xs text-muted">
                {m}
              </p>
            ))}
          </div>
          {it.providerId && it.model && (
            <Button
              variant="ghost"
              className="text-xs"
              busy={testing === `${it.providerId}/${it.model}`}
              disabled={!!testing}
              onClick={() => test(it.providerId!, it.model!)}
              title="Runs one small request to check speed and JSON output (uses the model once)"
            >
              Test
            </Button>
          )}
        </div>
      ))}
      <ErrorNote error={error} />
    </div>
  );
}

function SetupForm() {
  const navigate = useNavigate();
  const books = useBooks();
  const personas = useQuery({ queryKey: ["personas"], queryFn: api.personas, staleTime: Infinity });
  const { track, tracks } = useGoalTrack();
  const [source, setSource] = useState<InterviewSource>("chapter");
  const [fp, setFp] = useState<string>("");
  const [chapterIdx, setChapterIdx] = useState<number | null>(null);
  const [trackId, setTrackId] = useState<string | null>(null);
  const [topicIds, setTopicIds] = useState<string[]>([]);
  const [level, setLevel] = useState<Level>("mid");
  const [minutes, setMinutes] = useState(15);
  const [personaId, setPersonaId] = useState<string | null>(null);
  const [mode, setMode] = useState<"auto" | "fast" | "thorough">("auto");
  const [confirmWeak, setConfirmWeak] = useState(false);
  const check = useQuery({ queryKey: ["interviewCheck", minutes], queryFn: () => api.interviewCheck(minutes) });
  const blocking = !!check.data?.blocking;
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState("");
  const [stream, setStream] = useState("");
  const [error, setError] = useState<string | null>(null);

  const usableBooks = (books.data ?? []).filter((b) => b.path && b.status !== "outdated");
  const chapters = useQuery({
    queryKey: ["chapters", fp],
    queryFn: () => api.bookChapters(fp),
    enabled: !!fp && source === "chapter",
  });
  const activeTrack = trackId ?? track;
  const skills = useQuery({
    queryKey: ["skills", activeTrack],
    queryFn: () => api.skills(activeTrack),
    enabled: source === "topics" || source === "general",
  });

  const needsBook = source === "chapter" || source === "book";
  const ready = needsBook ? !!fp && (source === "book" || chapterIdx != null) : true;

  async function start() {
    setBusy(true);
    setError(null);
    setStream("");
    const setup: InterviewSetup = {
      source,
      bookFingerprint: needsBook ? fp : null,
      chapterIdx: source === "chapter" ? chapterIdx : null,
      track: needsBook ? null : activeTrack,
      topicIds: needsBook ? [] : topicIds,
      level,
      minutes,
      personaId,
      mode,
    };
    try {
      const iv = await api.startInterview(newRequestId(), setup, (e) => {
        if (e.type === "status") setStatus(e.message);
        else setStream((s) => s + e.text);
      });
      navigate(`/interview/${iv.id}`);
    } catch (e) {
      setError(errorMessage(e));
      setBusy(false);
    }
  }

  if (busy) {
    return (
      <section className="rounded-lg border border-line bg-panel p-5">
        <StreamBox status={status || "Preparing your interview…"} text={stream} />
        <p className="mt-2 text-xs text-muted">
          The interviewer is reading the material and planning questions and the key points to check.
        </p>
      </section>
    );
  }

  return (
    <section className="rounded-lg border border-line bg-panel p-5">
      <h2 className="mb-3 text-base font-semibold">New mock interview</h2>

      <div className="mb-4 grid gap-2 sm:grid-cols-2 lg:grid-cols-4">
        {SOURCES.map((s) => (
          <button
            key={s.id}
            onClick={() => setSource(s.id)}
            className={`rounded-lg border p-3 text-left ${s.id === source ? "border-accent bg-accent-soft" : "border-line hover:border-accent"}`}
          >
            <div className="text-sm font-medium">{s.label}</div>
            <div className="text-xs text-muted">{s.hint}</div>
          </button>
        ))}
      </div>

      {needsBook && (
        <div className="grid gap-3 sm:grid-cols-2">
          <Field label="Book">
            <select
              className={inputClass}
              value={fp}
              onChange={(e) => {
                setFp(e.target.value);
                setChapterIdx(null);
              }}
            >
              <option value="">Choose a book…</option>
              {usableBooks.map((b) => (
                <option key={b.fingerprint} value={b.fingerprint}>
                  {b.title}
                </option>
              ))}
            </select>
          </Field>
          {source === "chapter" && (
            <Field
              label="Chapter"
              hint={
                fp && chapters.data?.length === 0 ? "Open this book once so the app can read its chapters." : undefined
              }
            >
              <select
                className={inputClass}
                value={chapterIdx ?? ""}
                disabled={!chapters.data?.length}
                onChange={(e) => setChapterIdx(e.target.value === "" ? null : Number(e.target.value))}
              >
                <option value="">Choose a chapter…</option>
                {chapters.data?.map((c) => (
                  <option key={c.idx} value={c.idx}>
                    {c.title} (p. {c.startPage})
                  </option>
                ))}
              </select>
            </Field>
          )}
        </div>
      )}

      {!needsBook && (
        <div className="mb-3">
          <Field label="Track">
            <select
              className={`${inputClass} w-auto`}
              value={activeTrack}
              onChange={(e) => {
                setTrackId(e.target.value);
                setTopicIds([]);
              }}
            >
              {tracks.map((t) => (
                <option key={t.id} value={t.id}>
                  {t.label}
                </option>
              ))}
            </select>
          </Field>
          <div className="mb-1 text-xs font-medium text-muted">
            Topics{" "}
            {topicIds.length === 0 && <span className="font-normal">— none picked: your weakest topics are used</span>}
          </div>
          <div className="flex flex-wrap gap-1.5">
            {skills.data?.topics.map((t) => {
              const on = topicIds.includes(t.id);
              return (
                <button
                  key={t.id}
                  onClick={() => setTopicIds(on ? topicIds.filter((x) => x !== t.id) : [...topicIds, t.id])}
                  className={`rounded-full border px-2.5 py-0.5 text-xs ${on ? "border-accent bg-accent-soft text-accent" : "border-line text-muted hover:text-ink"}`}
                >
                  {t.label}
                  {t.mastery != null && ` · ${t.mastery}%`}
                </button>
              );
            })}
          </div>
        </div>
      )}

      <div className="mt-2 flex flex-wrap gap-6">
        <Field label="Level">
          <Segmented
            value={level}
            onChange={setLevel}
            options={[
              { id: "junior", label: "Junior" },
              { id: "mid", label: "Mid" },
              { id: "senior", label: "Senior" },
            ]}
          />
        </Field>
        <Field label="Length">
          <Segmented
            value={minutes}
            onChange={setMinutes}
            options={[
              { id: 15, label: "15 min" },
              { id: 30, label: "30 min" },
              { id: 45, label: "45 min" },
            ]}
          />
        </Field>
      </div>

      <div className="mb-1 text-xs font-medium text-muted">Interviewer</div>
      <div className="mb-4 grid gap-2 sm:grid-cols-2 lg:grid-cols-4">
        <button
          onClick={() => setPersonaId(null)}
          className={`flex items-center gap-2 rounded-lg border p-2.5 text-left ${personaId === null ? "border-accent bg-accent-soft" : "border-line hover:border-accent"}`}
        >
          <span className="inline-flex size-9 items-center justify-center rounded-full bg-sunken">
            <Shuffle className="size-4 text-muted" />
          </span>
          <span>
            <span className="block text-sm font-medium">Surprise me</span>
            <span className="block text-xs text-muted">Someone new each time</span>
          </span>
        </button>
        {personas.data?.map((p) => (
          <button
            key={p.id}
            onClick={() => setPersonaId(p.id)}
            title={`${p.style}\n\n${p.probing}`}
            className={`flex items-center gap-2 rounded-lg border p-2.5 text-left ${personaId === p.id ? "border-accent bg-accent-soft" : "border-line hover:border-accent"}`}
          >
            <Avatar name={p.name} />
            <span className="min-w-0">
              <span className="block truncate text-sm font-medium">{p.name}</span>
              <span className="block truncate text-xs text-muted">{p.role}</span>
            </span>
          </button>
        ))}
      </div>

      <div className="mb-4 grid gap-4 rounded-lg border border-line p-3 md:grid-cols-2">
        <div>
          <div className="mb-2 text-xs font-medium text-muted">Models for this interview</div>
          <ModelCheckPanel minutes={minutes} />
        </div>
        <div>
          <div className="mb-2 text-xs font-medium text-muted">How the interviewer thinks</div>
          <Segmented
            value={mode}
            onChange={setMode}
            options={[
              { id: "auto", label: `Auto (${check.data?.recommendedMode ?? "…"})` },
              { id: "thorough", label: "Thorough" },
              { id: "fast", label: "Fast" },
            ]}
          />
          <p className="mt-2 text-xs text-muted">
            <b>Thorough:</b> a strict judge reads each answer (and whether it's an answer at all, or a clarifying
            question), then the interviewer speaks — two model calls, fairer and more in character. <b>Fast:</b> one
            call does both — about half the wait on a slow local model.
          </p>
        </div>
      </div>

      <ErrorNote error={error} />
      {blocking && (
        <label className="mt-2 flex items-center gap-2 text-sm text-bad">
          <input type="checkbox" checked={confirmWeak} onChange={(e) => setConfirmWeak(e.target.checked)} />A model
          above isn't up to this. Start anyway (results may be unreliable).
        </label>
      )}
      <div className="mt-3 flex items-center gap-3">
        <Button variant="primary" disabled={!ready || (blocking && !confirmWeak)} onClick={start}>
          Start interview
        </Button>
        <span className="text-xs text-muted">
          The interviewer keeps coming back to each key point until you've clearly shown it — expect follow-ups.
        </span>
      </div>
    </section>
  );
}

export default function InterviewsPage() {
  const navigate = useNavigate();
  const list = useQuery({ queryKey: ["interviews"], queryFn: api.interviews });

  return (
    <div className="h-full overflow-y-auto">
      <header className="sticky top-0 z-10 border-b border-line bg-bg/95 px-6 py-3 backdrop-blur">
        <h1 className="text-lg font-semibold">Interviews</h1>
      </header>
      <div className="mx-auto max-w-5xl px-6 py-5">
        <SetupForm />
        <h2 className="mt-8 mb-3 text-base font-semibold">Past interviews</h2>
        {list.isLoading && <Spinner />}
        {list.error && <ErrorNote error={errorMessage(list.error)} />}
        {list.data?.length === 0 && (
          <Empty title="No interviews yet">Your interviews and their reports are kept here.</Empty>
        )}
        {list.data && list.data.length > 0 && (
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-line text-left text-xs text-muted">
                <th className="py-2 font-medium">Subject</th>
                <th className="py-2 font-medium">Interviewer</th>
                <th className="py-2 font-medium">Level</th>
                <th className="py-2 font-medium">Result</th>
                <th className="py-2 font-medium">Date</th>
              </tr>
            </thead>
            <tbody>
              {list.data.map((iv) => (
                <tr
                  key={iv.id}
                  className="cursor-pointer border-b border-line hover:bg-panel"
                  onClick={() => navigate(`/interview/${iv.id}`)}
                >
                  <td className="py-2 pr-3">{iv.title}</td>
                  <td className="py-2 pr-3 text-muted">{iv.personaName}</td>
                  <td className="py-2 pr-3 text-muted capitalize">{iv.level}</td>
                  <td className="py-2 pr-3">
                    {iv.verdict ? (
                      <Badge tone={verdictTone[iv.verdict]}>
                        {verdictLabel[iv.verdict]} · {iv.overall}
                      </Badge>
                    ) : (
                      <Badge>
                        in progress · {iv.asked}/{iv.planned}
                      </Badge>
                    )}
                  </td>
                  <td className="py-2 text-muted">{formatDate(iv.createdAt)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}
