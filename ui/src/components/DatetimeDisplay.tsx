import { createMemo, createSignal, onCleanup } from "solid-js";

const justNowSeconds = 60;
const maxRelativeHours = 12;
const futureSlackMs = 10000;
const maxRelativeMs = maxRelativeHours * 60 * 60 * 1000;

interface DatetimeDisplayProps {
  date: Date;
  locale?: string;
}

export default function DatetimeDisplay(props: DatetimeDisplayProps) {
  const locale = props.locale ?? navigator.language;

  const rtf = new Intl.RelativeTimeFormat(locale, { numeric: "auto" });
  const dtf = new Intl.DateTimeFormat(locale, {
    dateStyle: "medium",
    timeStyle: "short",
  });

  const [now, setNow] = createSignal(Date.now());

  const timer = setInterval(() => {
    setNow(Date.now());
  }, 1000);

  onCleanup(() => clearInterval(timer));

  const text = createMemo(() => {
    const date = props.date.getTime();
    const diff = now() - date;

    // future date
    if (diff < -futureSlackMs) {
      return dtf.format(props.date);
    }

    // older than threshold
    if (diff > maxRelativeMs) {
      return dtf.format(props.date);
    }

    const seconds = Math.floor(diff / 1000);
    if (seconds < justNowSeconds) {
      return "just now";
    }

    const minutes = Math.floor(seconds / 60);
    if (minutes < 60) {
      return rtf.format(-minutes, "minute");
    }

    const hours = Math.floor(minutes / 60);
    return rtf.format(-hours, "hour");
  });

  return <span>{text()}</span>;
}
