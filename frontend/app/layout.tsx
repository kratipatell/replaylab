import type { Metadata } from "next";
import Sidebar from "@/components/Sidebar";
import "./globals.css";

export const metadata: Metadata = {
  title: "Replaylab",
};

export default function RootLayout({
  children,
}: Readonly<{ children: React.ReactNode }>) {
  return (
    <html lang="en">
      <body className="antialiased">
        <div className="flex h-screen overflow-hidden bg-[var(--background)] text-[var(--foreground)]">
          <Sidebar />
          <main className="min-w-0 flex-1 overflow-y-auto p-6">{children}</main>
        </div>
      </body>
    </html>
  );
}
