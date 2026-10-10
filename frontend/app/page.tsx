import AgentsCard from "@/components/AgentsCard";

export default function Home() {
  return (
    <div className="flex flex-col gap-4">
      <h1 className="text-xl font-bold">Home</h1>
      <AgentsCard />
    </div>
  );
}
