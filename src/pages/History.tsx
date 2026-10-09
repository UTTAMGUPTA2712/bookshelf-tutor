import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "react-router";
import { api, errorMessage } from "../ipc";
import { Badge, Empty, ErrorNote, formatDate, scoreTone, Spinner } from "../ui";

export default function HistoryPage() {
  const navigate = useNavigate();
  const attempts = useQuery({ queryKey: ["attempts"], queryFn: () => api.attempts() });

  return (
    <div className="h-full overflow-y-auto">
      <header className="sticky top-0 z-10 border-b border-line bg-bg/95 px-6 py-3 backdrop-blur">
        <h1 className="text-lg font-semibold">Quizzes</h1>
      </header>
      <div className="px-6 py-4">
        {attempts.isLoading && <Spinner />}
        {attempts.error && <ErrorNote error={errorMessage(attempts.error)} />}
        {attempts.data?.length === 0 && (
          <Empty title="No quizzes yet">Open a book, go to a chapter you've read, and click “Quiz me”.</Empty>
        )}
        {attempts.data && attempts.data.length > 0 && (
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-line text-left text-xs text-muted">
                <th className="py-2 font-medium">Chapter</th>
                <th className="py-2 font-medium">Book</th>
                <th className="py-2 font-medium">Score</th>
                <th className="py-2 font-medium">Date</th>
              </tr>
            </thead>
            <tbody>
              {attempts.data.map((a) => (
                <tr
                  key={a.id}
                  className="cursor-pointer border-b border-line hover:bg-panel"
                  onClick={() => navigate(a.kind === "assessment" ? `/assessment/${a.id}` : `/quiz/${a.id}`)}
                >
                  <td className="py-2 pr-3">
                    {a.kind === "assessment" && (
                      <span className="mr-1.5">
                        <Badge tone="accent">skill test</Badge>
                      </span>
                    )}
                    {a.chapterTitle}
                  </td>
                  <td className="py-2 pr-3 text-muted">{a.bookTitle ?? "—"}</td>
                  <td className="py-2 pr-3">
                    {a.score != null ? (
                      <Badge tone={scoreTone(a.score)}>{a.score}</Badge>
                    ) : (
                      <Badge>
                        in progress · {a.answered}/{a.questionCount}
                      </Badge>
                    )}
                  </td>
                  <td className="py-2 text-muted">{formatDate(a.createdAt)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}
