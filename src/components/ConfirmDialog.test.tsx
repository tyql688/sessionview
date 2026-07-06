import { describe, it, expect, vi } from "vitest";
import { render, fireEvent } from "@testing-library/react";
import { ConfirmDialog } from "./ConfirmDialog";

describe("ConfirmDialog", () => {
  it("renders nothing when closed", () => {
    const { queryByRole } = render(
      <ConfirmDialog
        open={false}
        title="Delete session"
        message="Are you sure?"
        confirmLabel="Delete"
        onConfirm={() => {}}
        onCancel={() => {}}
      />,
    );
    expect(queryByRole("alertdialog")).toBeNull();
  });

  it("renders title, message and confirm label when open", () => {
    const { getByRole, getByText } = render(
      <ConfirmDialog
        open={true}
        title="Delete session"
        message="Are you sure?"
        confirmLabel="Delete"
        onConfirm={() => {}}
        onCancel={() => {}}
      />,
    );
    const dialog = getByRole("alertdialog");
    expect(dialog).toBeInTheDocument();
    expect(getByText("Delete session")).toBeInTheDocument();
    expect(getByText("Are you sure?")).toBeInTheDocument();
    expect(getByText("Delete")).toBeInTheDocument();
  });

  it("invokes onConfirm when the confirm button is clicked", () => {
    const onConfirm = vi.fn();
    const { getByText } = render(
      <ConfirmDialog
        open={true}
        title="Delete session"
        message="Are you sure?"
        confirmLabel="Delete"
        onConfirm={onConfirm}
        onCancel={() => {}}
      />,
    );
    fireEvent.click(getByText("Delete"));
    expect(onConfirm).toHaveBeenCalledTimes(1);
  });

  it("applies the danger class to the confirm button when danger is set", () => {
    const { getByText } = render(
      <ConfirmDialog
        open={true}
        title="Delete session"
        message="Are you sure?"
        confirmLabel="Delete"
        danger={true}
        onConfirm={() => {}}
        onCancel={() => {}}
      />,
    );
    // The destructive variant carries the destructive token classes.
    expect(getByText("Delete").className).toContain("destructive");
  });
});
