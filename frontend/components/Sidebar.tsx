"use client";

import { useEffect, useState } from "react";
import Link from "next/link";
import {
  Bot,
  ChevronDown,
  ChevronsLeft,
  ChevronsRight,
  Home,
  Plus,
  type LucideIcon,
} from "lucide-react";
import { agents } from "@/lib/agents";

type NavItem = { label: string; href: string; icon: LucideIcon };

const NAV: NavItem[] = [{ label: "Home", href: "/", icon: Home }];

export default function Sidebar() {
  const [collapsed, setCollapsed] = useState(false);
  const [agentsOpen, setAgentsOpen] = useState(true);

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

      <button
        type="button"
        disabled
        title="Coming soon"
        className="flex w-full items-center justify-center gap-1 rounded bg-[var(--accent)] px-2 py-1.5 text-sm font-medium text-[var(--accent-foreground)] disabled:cursor-not-allowed disabled:opacity-50"
      >
        <Plus size={16} />
        {!collapsed && <span>New agent</span>}
      </button>

      <nav aria-label="Primary" className="flex w-full flex-col gap-1">
        {NAV.map((item) => (
          <Link
            key={item.href}
            href={item.href}
            aria-current="page"
            title={item.label}
            className={`flex items-center gap-2 rounded px-2 py-1.5 text-sm font-medium text-[var(--accent)] ${
              collapsed ? "justify-center" : ""
            }`}
          >
            <item.icon size={16} />
            {!collapsed && <span>{item.label}</span>}
          </Link>
        ))}
      </nav>

      <div className="flex w-full flex-col gap-1">
        <button
          type="button"
          aria-expanded={agentsOpen}
          onClick={() => setAgentsOpen((value) => !value)}
          className={`flex w-full items-center gap-1 rounded px-2 py-1.5 text-sm text-[var(--muted)] ${
            collapsed ? "justify-center" : "justify-between"
          }`}
        >
          <span className="flex items-center gap-1">
            <Bot size={16} />
            {!collapsed && <span>Agents</span>}
          </span>
          {!collapsed && (
            <ChevronDown
              size={16}
              className={agentsOpen ? "" : "-rotate-90"}
            />
          )}
        </button>
        {agentsOpen &&
          (collapsed ? null : agents.length === 0 ? (
            <p className="px-2 text-xs text-[var(--muted)]">No agents yet.</p>
          ) : (
            <ul className="flex flex-col gap-1 px-2">
              {agents.map((agent) => (
                <li key={agent.id} className="text-sm">
                  {agent.name}
                </li>
              ))}
            </ul>
          ))}
      </div>
    </aside>
  );
}
