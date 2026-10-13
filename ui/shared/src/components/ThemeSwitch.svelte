<script lang="ts">
  // A button that steps through the themes: follow the system, light and dark.
  import Icon, { type IconName } from "./Icon.svelte";
  import { setThemeChoice, theme, type ThemeChoice } from "../theme.svelte";

  /** The order the button steps through, with the icon and the label of each pick. */
  const choices: readonly { choice: ThemeChoice; icon: IconName; label: string }[] = [
    { choice: "system", icon: "contrast", label: "Theme follows the system" },
    { choice: "light", icon: "sun", label: "Light theme" },
    { choice: "dark", icon: "moon", label: "Dark theme" },
  ];

  const position = $derived(Math.max(0, choices.findIndex((entry) => entry.choice === theme.choice)));
  const current = $derived(choices[position]!);
  const next = $derived(choices[(position + 1) % choices.length]!);
</script>

<button
  type="button"
  class="icon-button large"
  aria-label={`${current.label}. Switch to: ${next.label.toLowerCase()}`}
  title={`${current.label}. Click for: ${next.label.toLowerCase()}`}
  onclick={() => setThemeChoice(next.choice)}
>
  <Icon name={current.icon} />
</button>
