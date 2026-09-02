import { Antigravity, Claude, Codex, Copilot, Cursor, DeepSeek, Grok, Kimi, Minimax, OpenCode } from "@lobehub/icons";
import type { JSX } from "react";
import type { Provider } from "@/lib/types";
import { getProviderColor } from "@/stores/providerSnapshots";

const DEFAULT_ICON_SIZE = 14;

// Custom SVGs for providers not in @lobehub/icons:
// - pi: no @lobehub brand icon exists.
// - cc-mirror: a Claude mirror, not a real brand — the Claude glyph tinted pink.
// - commandcode: official glyph shipped in Command Code's VS Code extension.
function PiIcon({ size }: { size: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 800 800" fill="none" xmlns="http://www.w3.org/2000/svg">
      <path
        fillRule="evenodd"
        clipRule="evenodd"
        d="M165.29 165.29H517.36V400H400V517.36H282.65V634.72H165.29V165.29ZM282.65 282.65V400H400V282.65H282.65Z"
        fill="currentColor"
      />
      <path d="M517.36 400H634.72V634.72H517.36V400Z" fill="currentColor" />
    </svg>
  );
}

function CcMirrorIcon({ size }: { size: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg">
      <path
        d="M4.709 15.955l4.72-2.647.08-.23-.08-.128H9.2l-.79-.048-2.698-.073-2.339-.097-2.266-.122-.571-.121L0 11.784l.055-.352.48-.321.686.06 1.52.103 2.278.158 1.652.097 2.449.255h.389l.055-.157-.134-.098-.103-.097-2.358-1.596-2.552-1.688-1.336-.972-.724-.491-.364-.462-.158-1.008.656-.722.881.06.225.061.893.686 1.908 1.476 2.491 1.833.365.304.145-.103.019-.073-.164-.274-1.355-2.446-1.446-2.49-.644-1.032-.17-.619a2.97 2.97 0 01-.104-.729L6.283.134 6.696 0l.996.134.42.364.62 1.414 1.002 2.229 1.555 3.03.456.898.243.832.091.255h.158V9.01l.128-1.706.237-2.095.23-2.695.08-.76.376-.91.747-.492.584.28.48.685-.067.444-.286 1.851-.559 2.903-.364 1.942h.212l.243-.242.985-1.306 1.652-2.064.73-.82.85-.904.547-.431h1.033l.76 1.129-.34 1.166-1.064 1.347-.881 1.142-1.264 1.7-.79 1.36.073.11.188-.02 2.856-.606 1.543-.28 1.841-.315.833.388.091.395-.328.807-1.969.486-2.309.462-3.439.813-.042.03.049.061 1.549.146.662.036h1.622l3.02.225.79.522.474.638-.079.485-1.215.62-1.64-.389-3.829-.91-1.312-.329h-.182v.11l1.093 1.068 2.006 1.81 2.509 2.33.127.578-.322.455-.34-.049-2.205-1.657-.851-.747-1.926-1.62h-.128v.17l.444.649 2.345 3.521.122 1.08-.17.353-.608.213-.668-.122-1.374-1.925-1.415-2.167-1.143-1.943-.14.08-.674 7.254-.316.37-.729.28-.607-.461-.322-.747.322-1.476.389-1.924.315-1.53.286-1.9.17-.632-.012-.042-.14.018-1.434 1.967-2.18 2.945-1.726 1.845-.414.164-.717-.37.067-.662.401-.589 2.388-3.036 1.44-1.882.93-1.086-.006-.158h-.055L4.132 18.56l-1.13.146-.487-.456.061-.746.231-.243 1.908-1.312-.006.006z"
        // Brand color from the theme token — the hardcoded hex ignored the
        // dark theme's lighter cc-mirror shade.
        fill="var(--cc-mirror)"
        fillRule="nonzero"
      />
    </svg>
  );
}

function CommandCodeIcon({ size }: { size: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 144 144" fill="none" xmlns="http://www.w3.org/2000/svg">
      <path
        fillRule="evenodd"
        clipRule="evenodd"
        d="M73.1399 5.87207H70.4648C53.6901 5.87207 41.6998 5.88454 32.5877 7.10962C23.6446 8.312 18.3583 10.5847 14.4715 14.4715C10.5847 18.3583 8.312 23.6446 7.10962 32.5877C5.88454 41.6998 5.87207 53.6901 5.87207 70.4648V73.1398C5.87207 89.9145 5.88454 101.905 7.10962 111.017C8.312 119.96 10.5847 125.246 14.4715 129.133C18.3583 133.02 23.6446 135.293 32.5877 136.495C41.6998 137.72 53.6901 137.733 70.4648 137.733H73.1398C89.9145 137.733 101.905 137.72 111.017 136.495C119.96 135.293 125.246 133.02 129.133 129.133C133.02 125.246 135.293 119.96 136.495 111.017C137.72 101.905 137.733 89.9146 137.733 73.1399V70.4648C137.733 53.6901 137.72 41.6998 136.495 32.5877C135.293 23.6446 133.02 18.3583 129.133 14.4715C125.246 10.5847 119.96 8.312 111.017 7.10962C101.905 5.88454 89.9146 5.87207 73.1399 5.87207ZM10.3193 10.3193C0 20.6387 0 37.2474 0 70.4648V73.1398C0 106.357 0 122.966 10.3193 133.285C20.6387 143.605 37.2474 143.605 70.4648 143.605H73.1398C106.357 143.605 122.966 143.605 133.285 133.285C143.605 122.966 143.605 106.357 143.605 73.1399V70.4648C143.605 37.2474 143.605 20.6387 133.285 10.3193C122.966 0 106.357 0 73.1399 0H70.4648C37.2474 0 20.6387 0 10.3193 10.3193Z"
        fill="currentColor"
      />
      <path
        d="M98.8049 27.6163C89.3295 27.6163 81.6214 35.3243 81.6214 44.7997V52.1641H61.9832V44.7997C61.9832 35.3243 54.2752 27.6163 44.7997 27.6163C35.3243 27.6163 27.6163 35.3243 27.6163 44.7997C27.6163 54.2752 35.3243 61.9832 44.7997 61.9832H52.1641V81.6214H44.7997C35.3243 81.6214 27.6163 89.3295 27.6163 98.8049C27.6163 108.28 35.3243 115.988 44.7997 115.988C54.2752 115.988 61.9832 108.28 61.9832 98.8049V91.4406H81.6214V98.8049C81.6214 108.28 89.3295 115.988 98.8049 115.988C108.28 115.988 115.988 108.28 115.988 98.8049C115.988 89.3295 108.28 81.6214 98.8049 81.6214H91.4406V61.9832H98.8049C108.28 61.9832 115.988 54.2752 115.988 44.7997C115.988 35.3243 108.28 27.6163 98.8049 27.6163ZM91.4406 52.1641V44.7997C91.4406 40.7248 94.73 37.4354 98.8049 37.4354C102.88 37.4354 106.169 40.7248 106.169 44.7997C106.169 48.8747 102.88 52.1641 98.8049 52.1641H91.4406ZM44.7997 52.1641C40.7248 52.1641 37.4354 48.8747 37.4354 44.7997C37.4354 40.7248 40.7248 37.4354 44.7997 37.4354C48.8747 37.4354 52.1641 40.7248 52.1641 44.7997V52.1641H44.7997ZM61.9832 81.6214V61.9832H81.6214V81.6214H61.9832ZM98.8049 106.169C94.73 106.169 91.4406 102.88 91.4406 98.8049V91.4406H98.8049C102.88 91.4406 106.169 94.73 106.169 98.8049C106.169 102.88 102.88 106.169 98.8049 106.169ZM44.7997 106.169C40.7248 106.169 37.4354 102.88 37.4354 98.8049C37.4354 94.73 40.7248 91.4406 44.7997 91.4406H52.1641V98.8049C52.1641 102.88 48.8747 106.169 44.7997 106.169Z"
        fill="currentColor"
      />
    </svg>
  );
}

// Provider brand logos. Mainstream providers use official @lobehub/icons
// colored variants (the app's provider colors match their brand colors); Pi and
// cc-mirror keep custom SVGs above. Kimi's and Grok's brand marks are
// black-on-light / white-on-dark, so they use the monochrome variant tinted
// by text-primary.
const PROVIDER_ICONS: Record<Provider, (size: number) => JSX.Element> = {
  claude: (size) => <Claude.Color size={size} />,
  codex: (size) => <Codex.Color size={size} />,
  antigravity: (size) => <Antigravity.Color size={size} />,
  // OpenCode + Cursor have no .Color variant in @lobehub/icons — use base.
  opencode: (size) => <OpenCode size={size} />,
  kimi: (size) => (
    <span style={{ color: "var(--text-primary)", display: "inline-flex" }}>
      <Kimi size={size} />
    </span>
  ),
  cursor: (size) => <Cursor size={size} />,
  "cc-mirror": (size) => <CcMirrorIcon size={size} />,
  pi: (size) => <PiIcon size={size} />,
  grok: (size) => (
    <span style={{ color: "var(--text-primary)", display: "inline-flex" }}>
      <Grok size={size} />
    </span>
  ),
  dsh: (size) => <DeepSeek.Color size={size} />,
  mcode: (size) => <Minimax.Color size={size} />,
  copilot: (size) => <Copilot.Color size={size} />,
  commandcode: (size) => <CommandCodeIcon size={size} />,
};

export function ProviderIcon(props: { provider: Provider; size?: number }) {
  const icon = PROVIDER_ICONS[props.provider];
  return icon ? icon(props.size ?? DEFAULT_ICON_SIZE) : <span>?</span>;
}

export function ProviderDot(props: { provider: Provider }) {
  return (
    <span className="provider-dot provider-logo" style={{ color: getProviderColor(props.provider) }}>
      <ProviderIcon provider={props.provider} />
    </span>
  );
}

export function UserIcon() {
  return (
    <svg width="14" height="14" fill="currentColor" viewBox="0 0 24 24">
      <path d="M12 12c2.7 0 4.8-2.1 4.8-4.8S14.7 2.4 12 2.4 7.2 4.5 7.2 7.2 9.3 12 12 12zm0 2.4c-3.2 0-9.6 1.6-9.6 4.8v2.4h19.2v-2.4c0-3.2-6.4-4.8-9.6-4.8z" />
    </svg>
  );
}
