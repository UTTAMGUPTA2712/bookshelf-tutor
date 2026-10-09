import { useQuery } from "@tanstack/react-query";
import { Link } from "react-router";
import { ArrowRight, BookOpen, Flame, Layers, Map as MapIcon } from "lucide-react";
import { api, errorMessage, type Dashboard } from "../ipc";
import { Badge, Button, ErrorNote, scoreTone, Spinner } from "../ui";

function Tile({ label, value, sub }: { label: string; value: React.ReactNode; sub?: React.ReactNode }) {
  return (
    <div className="rounded-lg border border-line bg-panel p-4">
      <div className="text-xs font-medium text-muted">{label}</div>
      <div className="mt-1 text-2xl font-semibold tabular-nums">{value}</div>
      {sub && <div className="mt-0.5 text-xs text-muted">{sub}</div>}
    </div>
  );
}

function minutes(m: number) {
  return m < 60 ? `${m} min` : `${Math.floor(m / 60)} h ${m % 60} min`;
}

const dayFmt = (d: string, opts: Intl.DateTimeFormatOptions) => new Date(`${d}T00:00:00`).toLocaleDateString(undefined, opts);

/** 12 weeks × 7 days, one hue: darker = more activity. */
function ActivityGrid({ days }: { days: Dashboard["activity"] }) {
  const score = (d: Dashboard["activity"][number]) => d.quizzes * 3 + d.reviews + Math.ceil(d.minutes / 10);
  const max = Math.max(1, ...days.map(score));
  const step = (v: number) => (v === 0 ? 0 : Math.min(4, Math.ceil((v / max) * 4)));
  const fills = ["bg-sunken", "bg-accent/25", "bg-accent/50", "bg-accent/75", "bg-accent"];
  // Pad the start so columns are Monday-first weeks.
  const firstDow = (new Date(`${days[0]?.day}T00:00:00`).getDay() + 6) % 7;
  const cells: (Dashboard["activity"][number] | null)[] = [...Array(firstDow).fill(null), ...days];
  const weeks: (typeof cells)[] = [];
  for (let i = 0; i < cells.length; i += 7) weeks.push(cells.slice(i, i + 7));
  return (
    <div>
      <div className="flex gap-[3px]">
        {weeks.map((w, i) => (
          <div key={i} className="flex flex-col gap-[3px]">
            {w.map((d, j) =>
              d ? (
                <div
                  key={j}
                  className={`size-3 rounded-[2px] ${fills[step(score(d))]}`}
                  title={`${dayFmt(d.day, { weekday: "short", month: "short", day: "numeric" })}: ${d.quizzes} quiz${d.quizzes === 1 ? "" : "zes"}, ${d.reviews} card${d.reviews === 1 ? "" : "s"}, ${d.minutes} min reading`}
                />
              ) : (
                <div key={j} className="size-3" />
              ),
            )}
          </div>
        ))}
      </div>
      <div className="mt-2 flex items-center gap-1 text-[11px] text-muted">
        Less
        {fills.map((f) => (
          <span key={f} className={`size-2.5 rounded-[2px] ${f}`} />
        ))}
        More
      </div>
    </div>
  );
}

/** Last quiz scores as thin bars on a 0–100 scale, oldest left. */
function ScoreTrend({ points }: { points: Dashboard["scoreTrend"] }) {
  if (points.length === 0) return <p className="text-sm text-muted">No finished chapter quizzes yet.</p>;
  const avg = Math.round(points.reduce((a, p) => a + p.score, 0) / points.length);
  return (
    <div>
      <div className="relative flex h-28 items-end gap-1 border-b border-line">
        <div className="pointer-events-none absolute inset-x-0 border-t border-dashed border-line" style={{ bottom: "70%" }}>
          <span className="absolute -top-4 right-0 text-[10px] text-muted">70 = chapter done</span>
        </div>
        {points.map((p) => (
          <Link
            key={p.id}
            to={`/quiz/${p.id}`}
            className="group flex h-full max-w-6 flex-1 items-end"
            title={`${p.title} — ${p.score}/100, ${new Date(p.date).toLocaleDateString()}`}
          >
            <div className="w-full rounded-t-[4px] bg-accent group-hover:opacity-80" style={{ height: `${Math.max(2, p.score)}%` }} />
          </Link>
        ))}
      </div>
      <p className="mt-2 text-xs text-muted">
        Last {points.length} quiz{points.length === 1 ? "" : "zes"} · average {avg} · latest{" "}
        <Badge tone={scoreTone(points[points.length - 1].score)}>{points[points.length - 1].score}</Badge>
      </p>
    </div>
  );
}

export default function DashboardPage() {
  const dash = useQuery({ queryKey: ["dashboard"], queryFn: api.dashboard });
  const d = dash.data;

  return (
    <div className="h-full overflow-y-auto">
      <header className="sticky top-0 z-10 border-b border-line bg-bg/95 px-6 py-3 backdrop-blur">
        <h1 className="text-lg font-semibold">Today</h1>
      </header>
      <div className="mx-auto max-w-5xl px-6 py-5">
        {dash.isLoading && <Spinner />}
        {dash.error && <ErrorNote error={errorMessage(dash.error)} />}
        {d && (
          <>
            <div className="mb-5 grid gap-3 md:grid-cols-2">
              <section className="rounded-lg border border-line bg-panel p-5">
                <div className="mb-2 flex items-center gap-2 text-sm font-semibold">
                  <Layers className="size-4 text-accent" /> Review cards
                </div>
                {d.cards.dueNow > 0 ? (
                  <>
                    <p className="mb-3 text-sm">
                      <span className="text-2xl font-semibold tabular-nums">{Math.min(d.cards.dueNow, d.dailyLimit)}</span>{" "}
                      card{d.cards.dueNow === 1 ? "" : "s"} due
                      {d.cards.dueNow > d.dailyLimit && (
                        <span className="text-muted"> (of {d.cards.dueNow}; daily limit {d.dailyLimit})</span>
                      )}
                    </p>
                    <Link to="/cards">
                      <Button variant="primary">Start review</Button>
                    </Link>
                  </>
                ) : (
                  <p className="text-sm text-muted">
                    {d.cards.reviewedToday > 0
                      ? `All done — ${d.cards.reviewedToday} reviewed today.`
                      : d.cards.total > 0
                        ? "Nothing due right now."
                        : "No cards yet. Missed quiz questions turn into cards automatically."}
                    {d.cards.dueTomorrow > 0 && ` ${d.cards.dueTomorrow} due tomorrow.`}
                  </p>
                )}
              </section>
              <section className="rounded-lg border border-line bg-panel p-5">
                <div className="mb-2 flex items-center gap-2 text-sm font-semibold">
                  <MapIcon className="size-4 text-accent" /> Next on your {d.goalLabel} roadmap
                </div>
                {d.nextStep ? (
                  <>
                    <Link
                      to={`/book/${d.nextStep.book.fingerprint}?page=${d.nextStep.chapter.startPage}`}
                      className="mb-1 block text-base font-medium hover:text-accent hover:underline"
                    >
                      {d.nextStep.chapter.title}
                    </Link>
                    <p className="mb-3 text-xs text-muted">
                      {d.nextStep.book.title} · pp. {d.nextStep.chapter.startPage}–{d.nextStep.chapter.endPage} ·{" "}
                      {d.nextStep.topicLabel}
                    </p>
                    <Link to="/roadmap" className="inline-flex items-center gap-1 text-sm text-accent hover:underline">
                      Full roadmap <ArrowRight className="size-3.5" />
                    </Link>
                  </>
                ) : (
                  <p className="text-sm text-muted">
                    No chapters scheduled yet.{" "}
                    <Link to="/roadmap" className="text-accent hover:underline">
                      Open the roadmap
                    </Link>{" "}
                    to find them in your books.
                  </p>
                )}
              </section>
            </div>

            <div className="mb-5 grid grid-cols-2 gap-3 md:grid-cols-4">
              <Tile
                label="Streak"
                value={
                  <span className="inline-flex items-center gap-1">
                    {d.streak.current}
                    <Flame className={`size-5 ${d.streak.activeToday ? "text-warn" : "text-muted"}`} />
                  </span>
                }
                sub={`day${d.streak.current === 1 ? "" : "s"} · best ${d.streak.longest}${d.streak.activeToday ? "" : " · study today to keep it"}`}
              />
              <Tile label="Reading this week" value={minutes(d.reading.week)} sub={`today ${minutes(d.reading.today)}`} />
              <Tile label="Cards" value={d.cards.total} sub={`${d.cards.reviewedToday} reviewed today`} />
              <Tile
                label="Quizzes & interviews"
                value={d.activity.reduce((a, x) => a + x.quizzes, 0)}
                sub="in the last 12 weeks"
              />
            </div>

            <div className="mb-5 grid gap-3 md:grid-cols-2">
              <section className="rounded-lg border border-line bg-panel p-5">
                <h2 className="mb-3 text-sm font-semibold">Activity</h2>
                <ActivityGrid days={d.activity} />
              </section>
              <section className="rounded-lg border border-line bg-panel p-5">
                <h2 className="mb-3 text-sm font-semibold">Quiz scores</h2>
                <ScoreTrend points={d.scoreTrend} />
              </section>
            </div>

            <div className="mb-5 grid gap-3 md:grid-cols-2">
              <section className="rounded-lg border border-line bg-panel p-5">
                <div className="mb-3 flex items-center">
                  <h2 className="text-sm font-semibold">Weakest topics · {d.goalLabel}</h2>
                  <Link to="/skills" className="ml-auto text-xs text-accent hover:underline">
                    Skill map
                  </Link>
                </div>
                {d.weakTopics.length === 0 ? (
                  <p className="text-sm text-muted">
                    {d.untestedTopics > 0 ? (
                      <>
                        {d.untestedTopics} topics not measured yet —{" "}
                        <Link to="/skills" className="text-accent hover:underline">
                          take the skill test
                        </Link>
                        .
                      </>
                    ) : (
                      "No weak spots. Nice."
                    )}
                  </p>
                ) : (
                  <ul className="flex flex-col gap-2">
                    {d.weakTopics.map((t) => (
                      <li key={t.id} className="text-sm">
                        <div className="mb-1 flex justify-between">
                          <span>{t.label}</span>
                          <span className="tabular-nums text-muted">{t.mastery}%</span>
                        </div>
                        <div className="h-1.5 overflow-hidden rounded bg-sunken">
                          <div className={`h-full ${t.mastery < 50 ? "bg-bad" : "bg-warn"}`} style={{ width: `${t.mastery}%` }} />
                        </div>
                      </li>
                    ))}
                  </ul>
                )}
              </section>
              <section className="rounded-lg border border-line bg-panel p-5">
                <h2 className="mb-3 text-sm font-semibold">Books in progress</h2>
                {d.books.length === 0 ? (
                  <p className="text-sm text-muted">Open a book from the library to start.</p>
                ) : (
                  <ul className="flex flex-col gap-2.5">
                    {d.books.map((b) => {
                      const pct = b.pageCount ? Math.min(100, Math.round((b.lastPage / b.pageCount) * 100)) : 0;
                      return (
                        <li key={b.fingerprint}>
                          <Link to={`/book/${b.fingerprint}`} className="flex items-center gap-1.5 text-sm hover:underline">
                            <BookOpen className="size-3.5 shrink-0 text-accent" />
                            <span className="truncate">{b.title}</span>
                          </Link>
                          <div className="mt-1 flex items-center gap-2 text-[11px] text-muted">
                            <div className="h-1 flex-1 overflow-hidden rounded bg-sunken">
                              <div className="h-full bg-accent" style={{ width: `${pct}%` }} />
                            </div>
                            <span className="tabular-nums">
                              p. {b.lastPage}
                              {b.pageCount ? `/${b.pageCount}` : ""} · {b.chaptersDone} ch. done · {minutes(b.minutes)}
                            </span>
                          </div>
                        </li>
                      );
                    })}
                  </ul>
                )}
              </section>
            </div>

            <section className="rounded-lg border border-line bg-panel p-5">
              <h2 className="mb-1 text-sm font-semibold">AI usage</h2>
              <p className="mb-3 text-xs text-muted">Model calls recorded with your quizzes (question writing and grading).</p>
              {d.usage.length === 0 ? (
                <p className="text-sm text-muted">No AI calls yet.</p>
              ) : (
                <table className="w-full text-sm">
                  <thead>
                    <tr className="border-b border-line text-left text-xs text-muted">
                      <th className="py-1.5 font-medium">Model</th>
                      <th className="py-1.5 font-medium">Provider</th>
                      <th className="py-1.5 text-right font-medium">Calls</th>
                      <th className="py-1.5 text-right font-medium">Tokens in</th>
                      <th className="py-1.5 text-right font-medium">Tokens out</th>
                    </tr>
                  </thead>
                  <tbody>
                    {d.usage.map((u) => (
                      <tr key={`${u.provider}/${u.model}`} className="border-b border-line last:border-0">
                        <td className="py-1.5 font-mono text-xs">{u.model}</td>
                        <td className="py-1.5 text-muted">{u.provider}</td>
                        <td className="py-1.5 text-right tabular-nums">{u.calls}</td>
                        <td className="py-1.5 text-right tabular-nums">{u.promptTokens.toLocaleString()}</td>
                        <td className="py-1.5 text-right tabular-nums">{u.completionTokens.toLocaleString()}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              )}
            </section>
          </>
        )}
      </div>
    </div>
  );
}
