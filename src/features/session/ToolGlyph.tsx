import {
  AlarmClock,
  Bot,
  CalendarClock,
  CircleHelp,
  ClipboardCheck,
  ClipboardList,
  Code2,
  Database,
  FilePlus,
  FileSearch,
  FileText,
  Globe,
  Image,
  KeyRound,
  ListChecks,
  MousePointer2,
  Pencil,
  Plug,
  Puzzle,
  Repeat2,
  Search,
  Send,
  ShieldQuestion,
  Sparkles,
  Square,
  Table2,
  Target,
  Terminal,
  Trash2,
  Users,
  Wrench,
  type LucideIcon,
} from "lucide-react";
import type { ToolMetadata } from "@/lib/types";

interface ToolVisualKind {
  category: string;
  canonicalName: string;
}

const TOOL_ICONS: Record<string, LucideIcon> = {
  Agent: Bot,
  Apply_patch: Pencil,
  AskUserQuestion: CircleHelp,
  Bash: Terminal,
  ComputerUse: MousePointer2,
  CreateGoal: Target,
  CronCreate: CalendarClock,
  CronDelete: CalendarClock,
  CronList: CalendarClock,
  Delete: Trash2,
  DynamicTool: Puzzle,
  Edit: Pencil,
  FollowupTask: ClipboardList,
  GetGoal: Target,
  Glob: Search,
  Grep: Search,
  ImageGeneration: Image,
  JavaScript: Code2,
  Lint: Sparkles,
  ListAgents: Users,
  ListMcpResourcesTool: Plug,
  Plan: ClipboardList,
  Read: FileText,
  ReadMediaFile: Image,
  RequestPermissions: ShieldQuestion,
  ScheduleWakeup: AlarmClock,
  SendMessage: Send,
  SetGoalBudget: Target,
  Skill: Sparkles,
  SQL: Database,
  StructuredOutput: Table2,
  TaskCreate: ClipboardCheck,
  TaskList: ListChecks,
  TaskOutput: ClipboardList,
  TaskStop: Square,
  TaskUpdate: ClipboardCheck,
  ToolSearch: FileSearch,
  UpdateGoal: Target,
  WebFetch: Globe,
  WebSearch: Globe,
  Workflow: Repeat2,
  Write: FilePlus,
  mcp: Plug,
};

const CATEGORY_ICONS: Record<string, LucideIcon> = {
  agent: Bot,
  cron: CalendarClock,
  database: Database,
  file: FileText,
  goal: Target,
  interaction: KeyRound,
  mcp: Plug,
  media: Image,
  plan: ClipboardList,
  search: Search,
  shell: Terminal,
  skill: Sparkles,
  task: ClipboardList,
  tool: Wrench,
  web: Globe,
};

export function toolVisualKind(name: string, metadata?: ToolMetadata): ToolVisualKind {
  if (metadata?.category === "mcp" || name.startsWith("mcp__")) {
    return {
      canonicalName: metadata?.canonical_name ?? "mcp",
      category: "mcp",
    };
  }
  return {
    canonicalName: metadata?.canonical_name ?? name,
    category: metadata?.category ?? "unknown",
  };
}

export function ToolKindGlyph(props: ToolVisualKind & { className?: string }) {
  const Icon = TOOL_ICONS[props.canonicalName] ?? CATEGORY_ICONS[props.category] ?? Wrench;
  return <Icon className={props.className} aria-hidden="true" />;
}

export function ToolGlyph(props: { name: string; metadata?: ToolMetadata; className?: string }) {
  return <ToolKindGlyph {...toolVisualKind(props.name, props.metadata)} className={props.className} />;
}
