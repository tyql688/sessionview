import type React from "react";
import { useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { useI18n } from "../i18n/index";

export function InputDialog(props: {
  open: boolean;
  title: string;
  label: string;
  defaultValue: string;
  confirmLabel: string;
  maxLength?: number;
  onConfirm: (value: string) => void;
  onCancel: () => void;
}) {
  const { t } = useI18n();
  const [value, setValue] = useState(props.defaultValue);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (props.open) {
      setValue(props.defaultValue);
      // Focus input after the dialog portal renders
      requestAnimationFrame(() => {
        inputRef.current?.focus();
        inputRef.current?.select();
      });
    }
  }, [props.open, props.defaultValue]);

  function handleSubmit() {
    const trimmed = value.trim();
    if (trimmed && trimmed !== props.defaultValue) {
      props.onConfirm(trimmed);
    } else {
      props.onCancel();
    }
  }

  function handleKeyDown(e: React.KeyboardEvent<HTMLInputElement>) {
    if (e.key === "Enter") {
      e.preventDefault();
      handleSubmit();
    }
  }

  return (
    <Dialog
      open={props.open}
      onOpenChange={(open) => {
        if (!open) props.onCancel();
      }}
    >
      <DialogContent showCloseButton={false}>
        <DialogHeader>
          <DialogTitle>{props.title}</DialogTitle>
          <DialogDescription>{props.label}</DialogDescription>
        </DialogHeader>
        <div className="flex flex-col gap-1">
          <Input
            ref={inputRef}
            type="text"
            value={value}
            maxLength={props.maxLength}
            onChange={(e) => setValue(e.currentTarget.value)}
            onKeyDown={handleKeyDown}
          />
          {props.maxLength !== undefined && (
            <div className="self-end text-xs text-muted-foreground tabular-nums">
              {value.length}/{props.maxLength}
            </div>
          )}
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={props.onCancel}>
            {t("confirm.cancel")}
          </Button>
          <Button onClick={handleSubmit}>{props.confirmLabel}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
