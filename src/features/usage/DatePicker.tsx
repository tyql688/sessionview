import { Popover } from "@base-ui/react/popover";
import { CalendarDays, ChevronLeft, ChevronRight } from "lucide-react";
import { useEffect, useState } from "react";
import { buttonVariants } from "@/components/ui/button";
import { useI18n } from "@/i18n/index";
import { toLocalISODate } from "@/lib/formatters";
import { cn } from "@/lib/utils";

const CALENDAR_CELLS = 42;
const WEEKDAY_COLUMNS = [0, 1, 2, 3, 4, 5, 6];

function isoToDate(iso: string): Date {
  const [year, month, day] = iso.split("-").map(Number);
  return new Date(year, month - 1, day);
}

function monthStartFor(iso: string): Date {
  const date = isoToDate(iso);
  return new Date(date.getFullYear(), date.getMonth(), 1);
}

function addMonths(date: Date, months: number): Date {
  return new Date(date.getFullYear(), date.getMonth() + months, 1);
}

function sameMonth(left: Date, right: Date): boolean {
  return left.getFullYear() === right.getFullYear() && left.getMonth() === right.getMonth();
}

function formatTriggerDate(iso: string): string {
  return iso.replaceAll("-", "/");
}

export interface DatePickerProps {
  label: string;
  value: string;
  min?: string;
  max?: string;
  onChange: (value: string) => void;
}

export function DatePicker(props: DatePickerProps) {
  const { t, locale } = useI18n();
  const [open, setOpen] = useState(false);
  const [visibleMonth, setVisibleMonth] = useState(() => monthStartFor(props.value));

  const localeTag = locale === "zh" ? "zh-CN" : "en-US";
  const minMonth = props.min ? monthStartFor(props.min) : null;
  const maxMonth = props.max ? monthStartFor(props.max) : null;

  useEffect(() => {
    if (open) setVisibleMonth(monthStartFor(props.value));
  }, [open, props.value]);

  const monthLabel = new Intl.DateTimeFormat(localeTag, {
    month: "short",
    year: "numeric",
  }).format(visibleMonth);

  const firstVisible = new Date(visibleMonth);
  firstVisible.setDate(1 - visibleMonth.getDay());

  const days = Array.from({ length: CALENDAR_CELLS }, (_, index) => {
    const date = new Date(firstVisible);
    date.setDate(firstVisible.getDate() + index);
    return {
      date,
      iso: toLocalISODate(date),
      currentMonth: sameMonth(date, visibleMonth),
    };
  });

  const canGoPrevious = !minMonth || addMonths(visibleMonth, -1).getTime() >= minMonth.getTime();
  const canGoNext = !maxMonth || addMonths(visibleMonth, 1).getTime() <= maxMonth.getTime();

  const isDisabled = (iso: string): boolean =>
    (props.min !== undefined && iso < props.min) || (props.max !== undefined && iso > props.max);

  return (
    <Popover.Root open={open} onOpenChange={(next) => setOpen(next)}>
      <Popover.Trigger
        className={cn(
          buttonVariants({ variant: "outline", size: "sm" }),
          "usage-date-trigger h-auto min-w-0 active:translate-y-0",
        )}
        type="button"
        aria-label={props.label}
      >
        <CalendarDays aria-hidden="true" data-icon="inline-start" />
        <span>{formatTriggerDate(props.value)}</span>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Positioner className="isolate z-50" side="bottom" align="start" sideOffset={6}>
          <Popover.Popup
            className="usage-date-popover duration-100 data-open:animate-in data-open:fade-in-0 data-open:zoom-in-95 data-closed:animate-out data-closed:fade-out-0 data-closed:fill-mode-forwards data-closed:zoom-out-95"
            initialFocus={false}
          >
            <div className="usage-date-popover-head">
              <button
                className="usage-date-nav"
                type="button"
                aria-label={t("usage.previousMonth")}
                disabled={!canGoPrevious}
                onClick={() => setVisibleMonth((month) => addMonths(month, -1))}
              >
                <ChevronLeft aria-hidden="true" />
              </button>
              <div className="usage-date-month">{monthLabel}</div>
              <button
                className="usage-date-nav"
                type="button"
                aria-label={t("usage.nextMonth")}
                disabled={!canGoNext}
                onClick={() => setVisibleMonth((month) => addMonths(month, 1))}
              >
                <ChevronRight aria-hidden="true" />
              </button>
            </div>
            <div className="usage-date-weekdays" aria-hidden="true">
              {WEEKDAY_COLUMNS.map((day) => (
                <span key={day}>
                  {new Intl.DateTimeFormat(localeTag, {
                    weekday: "short",
                  }).format(new Date(2023, 0, 1 + day))}
                </span>
              ))}
            </div>
            <div className="usage-date-grid">
              {days.map((day) => {
                const selected = day.iso === props.value;
                return (
                  <Popover.Close
                    key={day.iso}
                    render={<button />}
                    className={cn("usage-date-day", !day.currentMonth && "is-outside", selected && "is-selected")}
                    type="button"
                    aria-pressed={selected}
                    disabled={isDisabled(day.iso)}
                    onClick={() => {
                      props.onChange(day.iso);
                    }}
                  >
                    {day.date.getDate()}
                  </Popover.Close>
                );
              })}
            </div>
          </Popover.Popup>
        </Popover.Positioner>
      </Popover.Portal>
    </Popover.Root>
  );
}
