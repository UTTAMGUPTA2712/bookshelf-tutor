import { useQuery, useQueryClient } from "@tanstack/react-query";
import { BookOpen, Gauge, History, Layers, Library as LibraryIcon, Map as MapIcon, Settings as SettingsIcon, Sun } from "lucide-react";
import { NavLink, Route, Routes } from "react-router";
import { open } from "@tauri-apps/plugin-dialog";
import { useState } from "react";
import { api, errorMessage } from "./ipc";
import { Button, ErrorNote, Spinner } from "./ui";
import LibraryPage from "./pages/Library";
import ReaderPage from "./pages/Reader";
import QuizPage from "./pages/Quiz";
import HistoryPage from "./pages/History";
import SkillsPage from "./pages/Skills";
import DashboardPage from "./pages/Dashboard";
import CardsPage from "./pages/Cards";
import RoadmapPage from "./pages/Roadmap";
import AssessmentPage from "./pages/Assessment";
import SettingsPage from "./pages/Settings";

export function useAppInfo() {
  return useQuery({ queryKey: ["appInfo"], queryFn: api.appInfo, staleTime: Infinity });
}

function Onboarding() {
  const qc = useQueryClient();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function choose() {
    const dir = await open({ directory: true, title: "Choose your books folder" });
    if (typeof dir !== "string") return;
    setBusy(true);
    setError(null);
    try {
      await api.setLibraryRoot(dir);
      await qc.invalidateQueries();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="flex h-full items-center justify-center p-8">
      <div className="max-w-md">
        <BookOpen className="mb-4 size-8 text-accent" />
        <h1 className="mb-2 text-2xl font-semibold">Welcome to Bookshelf Tutor</h1>
        <p className="mb-6 text-muted">
          Choose the folder that holds your PDF books. Nothing is uploaded — the app reads them where they are, and books
          you add later are copied there too.
        </p>
        <ErrorNote error={error} />
        <Button variant="primary" className="mt-4" busy={busy} onClick={choose}>
          {busy ? "Scanning books…" : "Choose books folder"}
        </Button>
      </div>
    </div>
  );
}

const nav = [
  { to: "/", label: "Today", icon: Sun },
  { to: "/library", label: "Library", icon: LibraryIcon },
  { to: "/cards", label: "Cards", icon: Layers },
  { to: "/skills", label: "Skills", icon: Gauge },
  { to: "/roadmap", label: "Roadmap", icon: MapIcon },
  { to: "/history", label: "Quizzes", icon: History },
  { to: "/settings", label: "Settings", icon: SettingsIcon },
];

export default function App() {
  const info = useAppInfo();
  const due = useQuery({
    queryKey: ["cards", "due"],
    queryFn: api.cardsDue,
    enabled: !!info.data?.libraryRoot,
    refetchInterval: 5 * 60_000,
  });
  const dueCount = due.data?.cards.length ?? 0;

  if (info.isLoading) {
    return (
      <div className="flex h-full items-center justify-center">
        <Spinner />
      </div>
    );
  }
  if (info.error) {
    return (
      <div className="p-8">
        <ErrorNote error={errorMessage(info.error)} />
      </div>
    );
  }
  if (!info.data?.libraryRoot) return <Onboarding />;

  return (
    <div className="flex h-full">
      <aside className="flex w-48 shrink-0 flex-col border-r border-line bg-panel">
        <div className="flex items-center gap-2 px-4 py-4 font-semibold">
          <BookOpen className="size-5 text-accent" />
          Bookshelf Tutor
        </div>
        <nav className="flex flex-col gap-0.5 px-2">
          {nav.map(({ to, label, icon: Icon }) => (
            <NavLink
              key={to}
              to={to}
              end={to === "/"}
              className={({ isActive }) =>
                `flex items-center gap-2 rounded-md px-2.5 py-1.5 text-sm ${isActive ? "bg-accent-soft font-medium text-accent" : "text-ink hover:bg-sunken"}`
              }
            >
              <Icon className="size-4" />
              {label}
              {to === "/cards" && dueCount > 0 && (
                <span className="ml-auto rounded-full bg-accent px-1.5 text-[10px] font-semibold text-accent-ink">
                  {dueCount}
                </span>
              )}
            </NavLink>
          ))}
        </nav>
        <div className="mt-auto px-4 py-3 text-[11px] text-muted">v{info.data.version}</div>
      </aside>
      <main className="relative min-w-0 flex-1 overflow-hidden">
        <Routes>
          <Route path="/" element={<DashboardPage />} />
          <Route path="/library" element={<LibraryPage />} />
          <Route path="/cards" element={<CardsPage />} />
          <Route path="/book/:fingerprint" element={<ReaderPage />} />
          <Route path="/quiz/:id" element={<QuizPage />} />
          <Route path="/history" element={<HistoryPage />} />
          <Route path="/skills" element={<SkillsPage />} />
          <Route path="/roadmap" element={<RoadmapPage />} />
          <Route path="/assessment/:id" element={<AssessmentPage />} />
          <Route path="/settings" element={<SettingsPage />} />
        </Routes>
      </main>
    </div>
  );
}
