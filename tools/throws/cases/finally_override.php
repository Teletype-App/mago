<?php
declare(strict_types=1);
namespace ThrowsProbe\finally_override;
function caller(): void { try { throw new \DomainException(); } finally { throw new \LengthException(); } }
