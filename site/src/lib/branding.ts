/**
 * Who the journey belongs to.
 *
 * The owner's name is personal, so it never lives in the source: it comes from
 * `PUBLIC_JOURNEY_OWNER` in the repo-root `.env` (git-ignored, see `.env.example`).
 * With nothing set the site is simply "The Journey", which is what a fresh clone shows.
 */
const owner = String(import.meta.env.PUBLIC_JOURNEY_OWNER ?? '').trim();

/** The configured owner, or `''` when the journey is anonymous. */
export const journeyOwner = owner;

/** Possessive first half of the title: `Ada’s` or `The`. */
export const journeyOwnerLabel = owner ? `${owner}’s` : 'The';

/** Full title used in `<title>`, the nav and the footer. */
export const journeyTitle = `${journeyOwnerLabel} Journey`;

/** `document.title` for a sub-page: `Lab — The Journey`. */
export function pageTitle(section?: string): string {
	return section ? `${section} — ${journeyTitle}` : journeyTitle;
}
