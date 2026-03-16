import { createSignal } from "solid-js";
import "./App.css";

import Circle from "./components/Circle";
import DatetimeDisplay from "./components/DatetimeDisplay";

export default function App() {
  const [connected, setConnected] = createSignal(false);
  const [currentStatus, setCurrentStatus] = createSignal<Record<
    string,
    any
  > | null>(null);
  const [lastUpdated, setLastUpdated] = createSignal<Date | null>(null);

  const evSource = new EventSource(`${location.origin}/events`);
  evSource.addEventListener("open", () => {
    setConnected(true);
  });
  evSource.addEventListener("message", (ev) => {
    try {
      const snapshot = JSON.parse(ev.data);
      setCurrentStatus(snapshot);
      setLastUpdated(new Date(snapshot.updated_unix_ms));
    } catch (err) {
      console.error(`failed to parse status JSON: ${err}`);
    }
  });
  evSource.addEventListener("error", (ev) => {
    console.error("event source error:", ev);
  });

  return (
    <>
      <section id="center">
        <h1>
          Visual Git Status
          <Circle
            color={connected() ? "rgb(110, 182, 114)" : "rgb(240, 146, 53)"}
          />
        </h1>
        <p>
          Repository: <code id="repo">-</code>
        </p>
        <p>
          Branch: <code id="branch">-</code>
        </p>
        <p>
          Last update:{" "}
          {lastUpdated() ? (
            <DatetimeDisplay date={lastUpdated()!} />
          ) : (
            <code>-</code>
          )}
        </p>

        <button
          onClick={async () => {
            try {
              const resp = await fetch(`${location.origin}/refresh`, {
                method: "POST",
              });
              if (!resp.ok) {
                console.error(await resp.text());
              }
            } catch (err) {
              console.error(err);
            }
          }}
        >
          Refresh Status
        </button>
        <p id="error"></p>
        <p>Current Status:</p>
        <code>{JSON.stringify(currentStatus())}</code>
      </section>
    </>
  );
}
