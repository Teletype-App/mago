<?php
namespace YiiProbe;
final class ValidationRecord extends \yii\db\ActiveRecord {
    public function rules(): array { return [['name', 'validateName']]; }
    public function validateName(): void { throw new \DomainException(); }
}
final class SaveRecord extends \yii\db\ActiveRecord {
    public function beforeSave(bool $insert): bool { throw new \LengthException(); }
}
final class EventRecord extends \yii\base\Component {
    public const EVENT_READY = 'ready';
    public function init(): void { $this->on(self::EVENT_READY, [$this, 'handle']); }
    public function handle(): void { throw new \UnexpectedValueException(); }
}
final class InitObject extends \yii\base\BaseObject {
    public function init(): void { throw new \OverflowException(); }
}
final class SetterModel extends \yii\base\Model {
    public int $real = 0;
    public function setVirtual(int $value): void { throw new \UnderflowException(); }
    public function setReal(int $value): void { throw new \DomainException(); }
}
final class RuleCallbacks extends \yii\base\Model {
    public function rules(): array { return [['name', [Validator::class, 'check']]]; }
}
final class Validator { public static function check(): void { throw new \RangeException(); } }
function validate(ValidationRecord $record): void { $record->validate(); }
function saveTrue(ValidationRecord $record): void { $record->save(); }
function saveFalse(ValidationRecord $record): void { $record->save(false); }
function saveNamedFalse(ValidationRecord $record): void { $record->save(runValidation: false); }
function saveHook(SaveRecord $record): void { $record->save(false); }
function event(EventRecord $record): void { $record->trigger(EventRecord::EVENT_READY); }
function registerOnly(EventRecord $record): void { $record->init(); }
function construct(): void { new InitObject(); }
function create(): void { \Yii::createObject(InitObject::class); }
function createConfig(): void { \Yii::createObject(['class' => InitObject::class]); }
function setter(SetterModel $record): void { $record->setAttributes(['virtual' => 1], false); }
function safeSetter(SetterModel $record): void { $record->setAttributes(['virtual' => 1]); }
function realProperty(SetterModel $record): void { $record->setAttributes(['real' => 1], false); }
function ruleCallback(RuleCallbacks $record): void { $record->validate(); }
function query(\yii\db\Query $query): void { $query->all(); }
