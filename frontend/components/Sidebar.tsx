"use client";

import { useEffect, useState } from "react";
import Link from "next/link";
import {
  Bot,
  ChevronsLeft,
  ChevronsRight,
  FlaskConical,
  Home,
  Plus,
  SearchCheck,
  type LucideIcon,
} from "lucide-react";

type NavItem = { label: string; href: string; icon: LucideIcon };

const NAV: NavItem[] = [
  { label: "Home", href: "/", icon: Home },
  { label: "Test Lab", href: "/test-lab", icon: FlaskConical },
  { label: "Check", href: "/check", icon: SearchCheck },
  { label: "Agents", href: "/agents", icon: Bot },
];

export default function Sidebar() {
  const [collapsed, setCollapsed] = useState(false);

  // Match the required auto-collapsed state on narrow viewports.
  useEffect(() => {
    const query = window.matchMedia("(max-width: 767px)");
    const sync = (): void => setCollapsed(query.matches);
    sync();
    query.addEventListener("change", sync);
    return () => query.removeEventListener("change", sync);
  }, []);

  return (
    <aside
      className={`flex h-full shrink-0 flex-col gap-4 border-r border-[var(--border)] bg-[var(--surface)] p-4 ${
        collapsed ? "w-16 items-center" : "w-60"
      }`}
    >
      <div className="flex w-full items-center justify-between gap-2">
        {collapsed ? (
          <span aria-hidden="true" className="text-sm font-bold">
            R
          </span>
        ) : (
          <span className="text-sm font-bold">Replaylab</span>
        )}
        <button
          type="button"
          aria-label={collapsed ? "Expand sidebar" : "Collapse sidebar"}
          onClick={() => setCollapsed((value) => !value)}
          className="rounded border border-[var(--border)] p-1 text-[var(--muted)]"
        >
          {collapsed ? <ChevronsRight size={16} /> : <ChevronsLeft size={16} />}
        </button>
      </div>

      {collapsed ? (
        <span className="rounded border border-[var(--border)] px-1 text-[10px] text-[var(--muted)]">
          P
        </span>
      ) : (
        <div className="flex flex-col gap-1">
          <span className="w-fit rounded border border-[var(--border)] px-1.5 py-0.5 text-[10px] font-semibold tracking-widest text-[var(--muted)]">
            PAPER
          </span>
          <p className="text-xs text-[var(--muted)]">Simulated money only.</p>
        </div>
      )}

      <Link
        href="/agents#new-agent"
        title="New agent"
        aria-label={collapsed ? "New agent" : undefined}
        className="flex min-h-10 w-full items-center justify-center gap-1 rounded bg-[var(--accent)] px-2 py-1.5 text-sm font-medium text-[var(--accent-foreground)] focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[var(--accent)]"
      >
        <Plus size={16} aria-hidden="true" />
        {!collapsed && <span>New agent</span>}
      </Link>

      <nav aria-label="Primary" className="flex w-full flex-col gap-1">
        {NAV.map((item) => {
          return (
            <Link
              key={item.href}
              href={item.href}
              title={item.label}
              className={`flex min-h-10 items-center gap-2 rounded px-2 py-1.5 text-sm font-medium text-[var(--muted)] hover:text-[var(--foreground)] focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[var(--accent)] ${collapsed ? "justify-center" : ""}`}
            >
              <item.icon size={16} aria-hidden="true" />
              {!collapsed && <span>{item.label}</span>}
            </Link>
          );
        })}
      </nav>
    </aside>
  );
}
