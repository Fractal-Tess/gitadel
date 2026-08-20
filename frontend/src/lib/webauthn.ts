function decodeBase64Url(value: string): ArrayBuffer {
	const padding = '='.repeat((4 - (value.length % 4)) % 4);
	const binary = atob(value.replace(/-/g, '+').replace(/_/g, '/') + padding);
	const bytes = Uint8Array.from(binary, (character) => character.charCodeAt(0));
	return bytes.buffer;
}

function encodeBase64Url(value: ArrayBuffer): string {
	const bytes = new Uint8Array(value);
	let binary = '';
	for (const byte of bytes) binary += String.fromCharCode(byte);
	return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/u, '');
}

function record(value: unknown, label: string): Record<string, unknown> {
	if (typeof value !== 'object' || value === null || Array.isArray(value)) {
		throw new Error(`${label} is missing from the passkey challenge.`);
	}
	return value as Record<string, unknown>;
}

function text(value: unknown, label: string): string {
	if (typeof value !== 'string') throw new Error(`${label} is missing from the passkey challenge.`);
	return value;
}

function descriptors(value: unknown): PublicKeyCredentialDescriptor[] | undefined {
	if (value === undefined) return undefined;
	if (!Array.isArray(value)) throw new Error('The passkey credential list is invalid.');
	return value.map((entry) => {
		const descriptor = record(entry, 'Credential');
		return {
			type: 'public-key',
			id: decodeBase64Url(text(descriptor.id, 'Credential ID')),
			transports: Array.isArray(descriptor.transports)
				? descriptor.transports.filter((transport): transport is AuthenticatorTransport =>
						['ble', 'cable', 'hybrid', 'internal', 'nfc', 'smart-card', 'usb'].includes(
							String(transport)
						)
					)
				: undefined
		};
	});
}

export function creationOptions(
	payload: Record<string, unknown>
): PublicKeyCredentialCreationOptions {
	const user = record(payload.user, 'Passkey user');
	const converted = {
		...payload,
		challenge: decodeBase64Url(text(payload.challenge, 'Challenge')),
		user: {
			...user,
			id: decodeBase64Url(text(user.id, 'User ID'))
		},
		excludeCredentials: descriptors(payload.excludeCredentials)
	};
	// The remaining fields were shape-checked by the API schema and are passed through unchanged.
	return converted as unknown as PublicKeyCredentialCreationOptions;
}

export function requestOptions(payload: Record<string, unknown>): PublicKeyCredentialRequestOptions {
	const converted = {
		...payload,
		challenge: decodeBase64Url(text(payload.challenge, 'Challenge')),
		allowCredentials: descriptors(payload.allowCredentials)
	};
	// The remaining fields were shape-checked by the API schema and are passed through unchanged.
	return converted as unknown as PublicKeyCredentialRequestOptions;
}

export async function createCredential(
	options: PublicKeyCredentialCreationOptions
): Promise<Record<string, unknown>> {
	const created = await navigator.credentials.create({ publicKey: options });
	if (!(created instanceof PublicKeyCredential)) throw new Error('Passkey creation was cancelled.');
	if (!(created.response instanceof AuthenticatorAttestationResponse)) {
		throw new Error('The browser returned an invalid passkey response.');
	}
	return {
		id: created.id,
		rawId: encodeBase64Url(created.rawId),
		type: created.type,
		response: {
			attestationObject: encodeBase64Url(created.response.attestationObject),
			clientDataJSON: encodeBase64Url(created.response.clientDataJSON),
			transports: created.response.getTransports?.() ?? []
		},
		extensions: created.getClientExtensionResults()
	};
}

export async function getCredential(
	options: PublicKeyCredentialRequestOptions
): Promise<Record<string, unknown>> {
	const received = await navigator.credentials.get({ publicKey: options });
	if (!(received instanceof PublicKeyCredential)) throw new Error('Passkey login was cancelled.');
	if (!(received.response instanceof AuthenticatorAssertionResponse)) {
		throw new Error('The browser returned an invalid passkey response.');
	}
	return {
		id: received.id,
		rawId: encodeBase64Url(received.rawId),
		type: received.type,
		response: {
			authenticatorData: encodeBase64Url(received.response.authenticatorData),
			clientDataJSON: encodeBase64Url(received.response.clientDataJSON),
			signature: encodeBase64Url(received.response.signature),
			userHandle: received.response.userHandle
				? encodeBase64Url(received.response.userHandle)
				: null
		},
		extensions: received.getClientExtensionResults()
	};
}
