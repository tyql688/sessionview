export interface DatePickerProps {
  label: string;
  value: string;
  min?: string;
  max?: string;
  onChange: (value: string) => void;
}

export function DatePicker(props: DatePickerProps) {
  return (
    <input
      className="usage-date-input"
      type="date"
      aria-label={props.label}
      value={props.value}
      min={props.min}
      max={props.max}
      onChange={(event) => {
        if (event.currentTarget.value) props.onChange(event.currentTarget.value);
      }}
    />
  );
}
