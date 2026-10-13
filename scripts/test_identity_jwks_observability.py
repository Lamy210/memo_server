from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[1]
AUTH_RS = ROOT / "backend" / "src" / "infrastructure" / "auth.rs"


class IdentityJwksObservabilitySourceContract(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.source = AUTH_RS.read_text(encoding="utf-8")

    def test_refresh_triggers_are_explicit_and_provider_neutral(self) -> None:
        for required in (
            "JwksRefreshTrigger::Scheduled",
            "JwksRefreshTrigger::UnknownKid",
            "JwksRefreshTrigger::InvalidSignature",
            "observe_jwks_refresh",
        ):
            self.assertIn(required, self.source)

    def test_jwks_logs_do_not_interpolate_transport_or_payload_errors(self) -> None:
        for forbidden in (
            "Failed to build authentication JWKS HTTP client: {error}",
            "Failed to build decoding key from JWKS: {error}",
            "Failed to fetch authentication JWKS: {error}",
            "Authentication JWKS body read failed: {error}",
            "Authentication JWKS response could not be decoded: {error}",
            "Authentication JWKS endpoint returned non-success status {}",
        ):
            self.assertNotIn(forbidden, self.source)


if __name__ == "__main__":
    unittest.main()
