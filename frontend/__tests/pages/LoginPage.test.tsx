// Copyright 2026 Appbird LLC. Licensed under "GNU Affero General Public License v3.0"

import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

// Stub the auth context + the SAML-status fetch the page calls on mount.
const { loginMock } = vi.hoisted(() => ({ loginMock: vi.fn() }));
vi.mock("../../src/state/useAuth", () => ({ useAuth: () => ({ login: loginMock }) }));
vi.mock("../../src/api/client", () => ({
    getSamlStatus: vi.fn().mockResolvedValue({
        enabled: false,
        label: "Sign in with SSO",
        local_login_disabled: false,
    }),
}));

import { LoginPage } from "../../src/pages/LoginPage";

// The form's <label>s aren't associated to inputs, so target by autocomplete.
const userInput = () => document.querySelector<HTMLInputElement>('input[autocomplete="username"]')!;
const passInput = () =>
    document.querySelector<HTMLInputElement>('input[autocomplete="current-password"]')!;

beforeEach(() => loginMock.mockReset());

describe("<LoginPage>", () => {
    it("renders the credential form", () => {
        render(<LoginPage />);
        expect(screen.getByText("Username or email")).toBeInTheDocument();
        expect(screen.getByText("Password")).toBeInTheDocument();
        expect(screen.getByRole("button", { name: "Sign in" })).toBeInTheDocument();
    });

    it("keeps submit disabled until both fields are filled", async () => {
        render(<LoginPage />);
        const submit = screen.getByRole("button", { name: "Sign in" });
        expect(submit).toBeDisabled();
        await userEvent.type(userInput(), "alice");
        await userEvent.type(passInput(), "secret");
        expect(submit).toBeEnabled();
    });

    it("calls login with the trimmed credentials on submit", async () => {
        loginMock.mockResolvedValue(undefined);
        render(<LoginPage />);
        await userEvent.type(userInput(), "  alice  ");
        await userEvent.type(passInput(), "secret");
        await userEvent.click(screen.getByRole("button", { name: "Sign in" }));
        expect(loginMock).toHaveBeenCalledWith("alice", "secret");
    });
});
