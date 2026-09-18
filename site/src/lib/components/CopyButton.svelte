<script lang="ts">
	let { text, label = 'Copy', class: klass = '' }: { text: string; label?: string; class?: string } = $props();
	let copied = $state(false);
	let timer: ReturnType<typeof setTimeout> | undefined;

	async function copy() {
		try {
			await navigator.clipboard.writeText(text);
		} catch {
			// clipboard blocked (insecure context, permissions) — fall back to a selection
			const ta = document.createElement('textarea');
			ta.value = text;
			ta.setAttribute('readonly', '');
			ta.style.position = 'fixed';
			ta.style.opacity = '0';
			document.body.appendChild(ta);
			ta.select();
			try {
				document.execCommand('copy');
			} catch {
				/* give up quietly */
			}
			ta.remove();
		}
		copied = true;
		clearTimeout(timer);
		timer = setTimeout(() => (copied = false), 1600);
	}
</script>

<button class="btn btn-sm {klass}" onclick={copy} aria-live="polite">
	{copied ? '✓ copied' : label}
</button>
